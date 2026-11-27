//! 16 §5 (2026-11-11): the browser rung of the scrape engine follows the local-network grant.
//! A restricted (capped) contact's rendered fetch is refused before obscura is asked; a guard
//! refusal never falls back to obscura; a granted rendered fetch (the owner's, or a full-share
//! contact's, Sky 20:28) of a loopback page still runs through the embedded browser. One test:
//! the browser actor is a process-global name.

#![cfg(not(target_os = "ios"))]

use kameo::actor::Spawn;
use scrape::{Scrape, ScrapeConfig};
use url::Url;
use web::fetch::Fetcher;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn the_browser_rung_follows_the_local_network_grant() {
    // SAFETY: first thing in the only test of this binary; nothing has read the environment.
    unsafe { std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1") };
    let cfg = browser::BrowserConfig::default();
    browser::warmup(&cfg);
    let actor = browser::Browsers::spawn(browser::Browsers::new(cfg));
    actor.register(browser::BROWSER_ACTOR).expect("register");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/router"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><body><h1>Router admin</h1><p>on the lan</p></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/router", server.uri())).unwrap();
    let scrape = |lan_twin: bool| {
        let fetcher = Fetcher::new("theuth-test", 2_097_152, 5)
            .and_then(|f| f.with_private_network(lan_twin))
            .expect("client");
        Scrape::new(
            fetcher,
            ScrapeConfig {
                concurrency: 1,
                ..ScrapeConfig::default()
            },
        )
    };
    let hits = || async { server.received_requests().await.unwrap_or_default().len() };

    // A restricted contact's `scrape_url rendered: true`: refused, nothing reaches the page.
    let err = tools::egress::scope(false, scrape(true).fetch_page(&url, true, false))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), web::actor::CONTACT_NO_RENDER);
    assert_eq!(hits().await, 0, "obscura never asked");

    // The guard refuses the loopback fetch (no local-network lane): no obscura fallback.
    let err = tools::egress::scope(true, scrape(false).fetch_page(&url, false, false))
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("is not allowed") && !text.contains("render:"),
        "{text}"
    );
    assert_eq!(hits().await, 0, "no fallback render");

    // The owner's rendered fetch of the same page still works.
    let page = tools::egress::scope(true, scrape(true).fetch_page(&url, true, false))
        .await
        .expect("owner render");
    assert_eq!(page.tier, "render");
    assert!(page.markdown.contains("on the lan"), "{}", page.markdown);
    assert!(hits().await >= 1);

    // A granted call's redirect loop is the redirect limit, not the guard: the browser still
    // gets to try the page (it asks the server beyond the fetcher's 1 + 5 hops).
    Mock::given(method("GET"))
        .and(path("/loop"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/loop"))
        .mount(&server)
        .await;
    let looping = Url::parse(&format!("{}/loop", server.uri())).unwrap();
    let before = hits().await;
    let r = tools::egress::scope(true, scrape(true).fetch_page(&looping, false, false)).await;
    if let Err(e) = &r {
        assert!(
            e.to_string().contains("render:"),
            "the browser was tried: {e}"
        );
    }
    assert!(
        hits().await - before > 6,
        "obscura asked after the fetcher gave up"
    );
}
