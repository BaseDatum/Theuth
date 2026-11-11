//! The `WreqChrome` client (design doc 122 R1-D1) against wiremock on
//! loopback, no network: the Chrome profile's headers go out, redirects are
//! returned not followed, bodies stream past the cap the `Fetcher` applies,
//! the SSRF guard refuses a blocked hostname before any connection, and the
//! configured timeouts fire.

use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use url::Url;
use web::fetch::client::{ClientConfig, ClientError, ClientKind, HttpClient, build};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MIB: usize = 1 << 20;

fn cfg() -> ClientConfig {
    ClientConfig {
        user_agent: "theuth/test".into(),
        accept_language: "de-DE,de;q=0.8".into(),
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(5),
        max_body: MIB,
        private: false,
    }
}

fn client(cfg: &ClientConfig) -> std::sync::Arc<dyn HttpClient> {
    let c = build(ClientKind::WreqChrome, cfg).expect("wreq client builds");
    assert_eq!(c.kind(), ClientKind::WreqChrome);
    c
}

fn url(server: &MockServer, p: &str) -> Url {
    Url::parse(&format!("{}{p}", server.uri())).unwrap()
}

#[tokio::test]
async fn sends_chrome_profile_headers_and_configured_accept_language() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/h"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;
    let cfg = cfg();
    let extra = [(
        HeaderName::from_static("x-theuth-test"),
        HeaderValue::from_static("1"),
    )];
    let resp = client(&cfg).get(&url(&server, "/h"), &extra).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.final_url, url(&server, "/h"));

    let reqs = server.received_requests().await.unwrap();
    let h = &reqs[0].headers;
    let get = |n: &str| h.get(n).map(|v| v.to_str().unwrap().to_owned());
    let ua = get("user-agent").expect("user-agent");
    assert!(ua.starts_with("Mozilla/5.0 (Macintosh;"), "{ua}");
    assert!(ua.contains("Chrome/148."), "{ua}");
    assert_ne!(
        ua, cfg.user_agent,
        "the honest UA never goes out on this arm"
    );
    assert!(
        get("sec-ch-ua")
            .unwrap()
            .contains(r#""Google Chrome";v="148""#),
        "{:?}",
        get("sec-ch-ua")
    );
    assert_eq!(get("sec-ch-ua-platform").as_deref(), Some(r#""macOS""#));
    assert_eq!(get("sec-ch-ua-mobile").as_deref(), Some("?0"));
    assert_eq!(get("sec-fetch-mode").as_deref(), Some("navigate"));
    assert_eq!(get("accept-language").as_deref(), Some("de-DE,de;q=0.8"));
    assert!(get("accept").unwrap().starts_with("text/html,"));
    assert_eq!(get("x-theuth-test").as_deref(), Some("1"));
    assert!(get("cookie").is_none());
}

#[tokio::test]
async fn empty_accept_language_keeps_the_profiles() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let cfg = ClientConfig {
        accept_language: String::new(),
        ..cfg()
    };
    client(&cfg).get(&url(&server, "/"), &[]).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[0].headers.get("accept-language").unwrap(),
        "en-US,en;q=0.9"
    );
}

#[tokio::test]
async fn returns_302_without_following() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/b"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let resp = client(&cfg()).get(&url(&server, "/a"), &[]).await.unwrap();
    assert_eq!(resp.status, 302);
    assert_eq!(resp.headers.get("location").unwrap(), "/b");
    assert_eq!(resp.final_url, url(&server, "/a"));
}

/// The `Fetcher`'s `read_capped` loop: drain, cut at `max`, flag.
async fn drain_capped(
    mut body: futures::stream::BoxStream<'static, Result<bytes::Bytes, ClientError>>,
    max: usize,
) -> (Vec<u8>, bool) {
    let mut raw = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = body.next().await {
        raw.extend_from_slice(&chunk.unwrap());
        if raw.len() > max {
            raw.truncate(max);
            truncated = true;
            break;
        }
    }
    (raw, truncated)
}

#[tokio::test]
async fn three_mib_body_streams_and_caps_like_the_fetcher() {
    let server = MockServer::start().await;
    let body = vec![b'x'; 3 * MIB];
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/plain"))
        .mount(&server)
        .await;
    let cfg = cfg();
    let c = client(&cfg);

    let resp = c.get(&url(&server, "/big"), &[]).await.unwrap();
    let (raw, truncated) = drain_capped(resp.body, 4 * MIB).await;
    assert!(!truncated);
    assert_eq!(raw.len(), 3 * MIB);

    let resp = c.get(&url(&server, "/big"), &[]).await.unwrap();
    let (raw, truncated) = drain_capped(resp.body, cfg.max_body).await;
    assert!(truncated);
    assert_eq!(raw.len(), cfg.max_body);
}

#[tokio::test]
async fn blocked_hostname_is_refused_by_the_guard_before_connecting() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    // `localhost` reaches the resolver (an IP literal would not) and resolves
    // to loopback, which the guard refuses; wiremock sees nothing.
    let target = Url::parse(&format!("http://localhost:{}/", server.address().port())).unwrap();
    let err = client(&cfg())
        .get(&target, &[])
        .await
        .err()
        .expect("must fail");
    let detail = err
        .as_rejected()
        .unwrap_or_else(|| panic!("not a guard refusal: {err}"));
    assert!(detail.contains("localhost"), "{detail}");
    assert!(err.to_string().starts_with("rejected: "));
}

#[tokio::test]
async fn read_timeout_fires_against_a_sleeping_mock() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let cfg = ClientConfig {
        read_timeout: Duration::from_millis(300),
        ..cfg()
    };
    let started = Instant::now();
    let err = client(&cfg)
        .get(&url(&server, "/"), &[])
        .await
        .err()
        .expect("must fail");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(matches!(err, ClientError::Transport(_)), "{err}");
    assert!(err.as_rejected().is_none());
}

#[tokio::test]
async fn connect_timeout_fires() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    // A zero connect budget: the non-blocking connect is pending on its first
    // poll and the deadline has already passed on the second.
    let cfg = ClientConfig {
        connect_timeout: Duration::from_nanos(1),
        ..cfg()
    };
    let err = client(&cfg)
        .get(&url(&server, "/"), &[])
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, ClientError::Transport(_)), "{err}");
    assert!(err.as_rejected().is_none());
}
