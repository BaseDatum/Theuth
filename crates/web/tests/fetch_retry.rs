//! Design doc 122 D2 (retries under one budget), D4 (default headers),
//! R1-D1 (the `Impersonate` dispatch against C's `WreqChrome` stub) and
//! D12 (post-inflate cap, `Content-Length` pre-check, sniff order) — all
//! through wiremock on loopback.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use web::fetch::client::{ClientKind, Impersonate};
use web::fetch::issue::IssueKind;
use web::fetch::{ACCEPT, FetchError, FetchFormat, Fetcher, PageOptions, STEALTH_NOT_BUILT};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const HTML: &str = include_str!("fixtures/fetch_sample.html");
const PDF: &[u8] = include_bytes!("fixtures/pdf-as-octet-stream.bin");

fn fetcher(server: &MockServer) -> Fetcher {
    Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::Never, true, Some("de-DE,de;q=0.8"))
}

/// A fetcher with a 1 s request timeout: total budget 2 s, connect 2 s → no second attempt fits.
fn short(server: &MockServer, timeout_ms: u64, connect_ms: u64) -> Fetcher {
    Fetcher::new_with_timeouts(
        "theuth-test",
        2_097_152,
        5,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(connect_ms),
    )
    .expect("client")
    .allowing(vec![*server.address()])
    .with_policy(Impersonate::Never, true, None)
}

/// Answers from `first` on the first call, then `rest`.
struct Sequence {
    calls: Arc<AtomicUsize>,
    first: ResponseTemplate,
    rest: ResponseTemplate,
}

impl Respond for Sequence {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first.clone()
        } else {
            self.rest.clone()
        }
    }
}

fn html_200() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(HTML.as_bytes(), "text/html; charset=utf-8")
}

fn wall_403() -> ResponseTemplate {
    ResponseTemplate::new(403)
        .insert_header("cf-mitigated", "challenge")
        .set_body_raw(
            b"<html><head><title>Just a moment...</title></head><body><p>Enable cookies.</p></body></html>".to_vec(),
            "text/html",
        )
}

async fn sequence(
    server: &MockServer,
    p: &str,
    first: ResponseTemplate,
    rest: ResponseTemplate,
) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(Sequence {
            calls: Arc::clone(&calls),
            first,
            rest,
        })
        .mount(server)
        .await;
    calls
}

#[tokio::test]
async fn transient_503_then_200_is_two_attempts() {
    let server = MockServer::start().await;
    let calls = sequence(&server, "/p", ResponseTemplate::new(503), html_200()).await;
    let start = Instant::now();
    let r = fetcher(&server)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("recovered");
    assert_eq!(r.attempts, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!r.ladder);
    assert!(r.issue.is_none());
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "1 s delay before the retry"
    );
    assert!(r.content.contains("Hello fetch world"));
}

#[tokio::test]
async fn non_retryable_500_is_one_attempt() {
    let server = MockServer::start().await;
    let calls = sequence(&server, "/p", ResponseTemplate::new(500), html_200()).await;
    let err = fetcher(&server)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, FetchError::Failed(m) if m == "HTTP 500"),
        "{err}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn connection_reset_then_200_is_retried() {
    // A listener that accepts and drops the first connection, then proxies nothing:
    // the second attempt goes to wiremock through a redirect-free second URL.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(html_200())
        .mount(&server)
        .await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let target = format!("{}/ok", server.uri());
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let mut stream = stream.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                drop(stream); // reset before any bytes
            } else {
                let body = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(body.as_bytes());
                break;
            }
        }
    });
    let r = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![addr, *server.address()])
        .with_policy(Impersonate::Never, true, None)
        .fetch(&format!("http://{addr}/first"), FetchFormat::Markdown, 4000)
        .await
        .expect("second attempt succeeded");
    assert_eq!(r.attempts, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(r.url.ends_with("/ok"));
}

#[tokio::test]
async fn retry_after_one_second_is_honoured() {
    let server = MockServer::start().await;
    let calls = sequence(
        &server,
        "/p",
        ResponseTemplate::new(429).insert_header("retry-after", "1"),
        html_200(),
    )
    .await;
    let start = Instant::now();
    let r = fetcher(&server)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("ok after the wait");
    assert_eq!(r.attempts, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn retry_after_beyond_the_budget_is_surfaced_not_waited() {
    let server = MockServer::start().await;
    let calls = sequence(
        &server,
        "/p",
        ResponseTemplate::new(429)
            .insert_header("retry-after", "120")
            .set_body_raw(b"<html><body>slow down</body></html>".to_vec(), "text/html"),
        html_200(),
    )
    .await;
    let start = Instant::now();
    let r = fetcher(&server)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("a rate limit is a diagnosed Ok");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(r.attempts, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let issue = r.issue.expect("issue");
    assert_eq!(issue.kind, IssueKind::RateLimited);
    assert_eq!(issue.retry_after, Some(Duration::from_secs(120)));
    assert_eq!(issue.detail, "HTTP 429");
    assert!(
        web::render::fetch_text(&r)
            .starts_with("ISSUE: rate_limited (HTTP 429) — next: retry in 120s")
    );
}

#[tokio::test]
async fn second_attempt_not_started_when_the_budget_cannot_fit_it() {
    // 1 s request timeout → 2 s total budget. The first attempt eats ~0.9 s,
    // the retry needs 1 s delay + 0.5 s connect > remaining: returned as is.
    let server = MockServer::start().await;
    let calls = sequence(
        &server,
        "/p",
        ResponseTemplate::new(503).set_delay(Duration::from_millis(900)),
        html_200(),
    )
    .await;
    let start = Instant::now();
    let r = short(&server, 1000, 500)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("503 is a diagnosed error page");
    assert!(
        start.elapsed() < Duration::from_millis(2500),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(r.attempts, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(r.issue.map(|i| i.kind), Some(IssueKind::ErrorPage));
}

#[tokio::test]
async fn budget_is_twice_the_timeout_and_a_sleeping_server_times_out_inside_it() {
    let server = MockServer::start().await;
    let f = short(&server, 1000, 200);
    assert_eq!(f.total_budget(), Duration::from_secs(2));
    Mock::given(method("GET"))
        .and(path("/sleep"))
        .respond_with(html_200().set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let start = Instant::now();
    let err = f
        .fetch(
            &format!("{}/sleep", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Failed(_)), "{err}");
    assert!(
        start.elapsed() < Duration::from_millis(2600),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn default_headers_on_the_reqwest_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/h"))
        .and(header("accept", ACCEPT))
        .and(header("accept-language", "de-DE,de;q=0.8"))
        .and(header("user-agent", "theuth-test"))
        .respond_with(html_200())
        .expect(1)
        .mount(&server)
        .await;
    let r = fetcher(&server)
        .fetch(&format!("{}/h", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("matched headers");
    assert_eq!(r.client, ClientKind::Reqwest);
    assert!(r.notes.is_empty(), "{:?}", r.notes);
}

#[tokio::test]
async fn impersonate_always_falls_back_to_reqwest_with_a_note_until_w5_lands() {
    let server = MockServer::start().await;
    let calls = sequence(&server, "/p", html_200(), html_200()).await;
    let r = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::Always, true, None)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("ok");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(r.attempts, 1);
    // Against C's stub: reqwest answered and said so. With W5 merged this
    // becomes `WreqChrome` and the note vanishes (closer checks).
    assert_eq!(r.client, ClientKind::Reqwest);
    assert_eq!(r.notes, vec![STEALTH_NOT_BUILT.to_owned()]);
}

#[tokio::test]
async fn on_challenge_escalates_once_on_a_diagnosed_403() {
    let server = MockServer::start().await;
    let calls = sequence(&server, "/p", wall_403(), html_200()).await;
    let r = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::OnChallenge, true, None)
        .fetch(&format!("{}/p", server.uri()), FetchFormat::Markdown, 4000)
        .await
        .expect("the second rung read the page");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(r.attempts, 2);
    assert!(r.ladder);
    assert!(r.issue.is_none());
    assert!(r.content.contains("Hello fetch world"));
    assert!(r.notes.contains(&STEALTH_NOT_BUILT.to_owned()));

    // Still a wall on the second rung: the first diagnosis is returned, attempts counted.
    let calls = sequence(&server, "/wall", wall_403(), wall_403()).await;
    let r = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::OnChallenge, true, None)
        .fetch(
            &format!("{}/wall", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .expect("diagnosed");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(r.attempts, 2);
    assert!(!r.ladder);
    assert_eq!(
        r.issue.map(|i| i.detail),
        Some("HTTP 403 · cloudflare".into())
    );
}

#[tokio::test]
async fn header_ladder_off_and_never_do_not_escalate() {
    let server = MockServer::start().await;
    for (mode, ladder) in [
        (Impersonate::OnChallenge, false),
        (Impersonate::Never, true),
    ] {
        let p = if ladder { "/never" } else { "/off" };
        let calls = sequence(&server, p, wall_403(), html_200()).await;
        let r = Fetcher::new("theuth-test", 2_097_152, 5)
            .expect("client")
            .allowing(vec![*server.address()])
            .with_policy(mode, ladder, None)
            .fetch(&format!("{}{p}", server.uri()), FetchFormat::Markdown, 4000)
            .await
            .expect("diagnosed");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{mode:?}");
        assert_eq!(r.attempts, 1);
        assert_eq!(r.client, ClientKind::Reqwest);
        assert_eq!(
            r.issue.as_ref().map(|i| i.kind),
            Some(IssueKind::AccessChallenge)
        );
        assert!(r.notes.is_empty(), "{mode:?}: {:?}", r.notes);
    }
    // A 403 JSON body (no issue) never escalates either.
    let calls = sequence(
        &server,
        "/json",
        ResponseTemplate::new(403).set_body_raw(br#"{"e":1}"#.to_vec(), "application/json"),
        html_200(),
    )
    .await;
    let err = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::OnChallenge, true, None)
        .fetch(
            &format!("{}/json", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "HTTP 403");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn request_keeps_one_attempt() {
    let server = MockServer::start().await;
    let calls = sequence(
        &server,
        "/api",
        ResponseTemplate::new(503),
        ResponseTemplate::new(200),
    )
    .await;
    let r = fetcher(&server)
        .request("GET", &format!("{}/api", server.uri()), &[], None)
        .await
        .expect("non-2xx is Ok on request");
    assert_eq!(r.status, 503);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn content_length_precheck_refuses_before_reading() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![b'a'; 4096], "text/plain"))
        .mount(&server)
        .await;
    let err = Fetcher::new("theuth-test", 1024, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::Never, true, None)
        .fetch(&format!("{}/big", server.uri()), FetchFormat::Text, 4000)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, FetchError::Failed(m) if m.starts_with("body too large (Content-Length 4096 > max 1024)")),
        "{err}"
    );
}

#[tokio::test]
async fn post_inflate_cap_sets_truncated() {
    let server = MockServer::start().await;
    // 200 KB of text compresses to a few hundred bytes: the wire body passes
    // the Content-Length check and the 64 KiB cap bites after inflation.
    let text = "<html><body><p>".to_owned() + &"inflate me. ".repeat(16_000) + "</p></body></html>";
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(text.as_bytes()).unwrap();
    let gz = enc.finish().unwrap();
    assert!(gz.len() < 64 * 1024);
    Mock::given(method("GET"))
        .and(path("/gz"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(gz, "application/gzip"))
        .mount(&server)
        .await;
    let f = Fetcher::new("theuth-test", 64 * 1024, 5)
        .expect("client")
        .allowing(vec![*server.address()])
        .with_policy(Impersonate::Never, true, None);
    let r = f
        .fetch_html(&format!("{}/gz", server.uri()), usize::MAX)
        .await
        .expect("inflated");
    assert!(r.truncated, "inflate hit the cap");
    assert_eq!(r.content.len(), 64 * 1024);
    assert!(r.content.starts_with("<html><body><p>inflate me."));
}

#[tokio::test]
async fn pdf_served_as_octet_stream_is_converted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(PDF.to_vec(), "application/octet-stream"),
        )
        .mount(&server)
        .await;
    // A zip-typed body that is not a container is still refused.
    Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 64], "application/zip"))
        .mount(&server)
        .await;
    let f = fetcher(&server);
    let r = f
        .fetch(
            &format!("{}/file", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .expect("sniffed as PDF and converted");
    assert!(
        r.content.contains("Page one of the octet-stream PDF"),
        "{}",
        r.content
    );
    assert!(r.content.contains("Page two says hello"), "{}", r.content);
    let err = f
        .fetch(
            &format!("{}/blob", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "unsupported content type: application/zip");
}

#[tokio::test]
async fn offset_and_compact_window_through_fetch_page() {
    let server = MockServer::start().await;
    let long = format!(
        "<html><head><title>Long</title></head><body><main>{}</main></body></html>",
        (1..=30)
            .map(|i| format!(
                "<h2>Part {i}</h2><p>{}</p>",
                "lorem ipsum dolor ".repeat(20)
            ))
            .collect::<String>()
    );
    Mock::given(method("GET"))
        .and(path("/long"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(long.into_bytes(), "text/html"))
        .mount(&server)
        .await;
    let f = fetcher(&server);
    let url = format!("{}/long", server.uri());
    let first = f
        .fetch_page(&url, FetchFormat::Markdown, PageOptions::new(1000))
        .await
        .expect("ok");
    assert!(first.truncated);
    let next = first.next_offset.expect("next_offset");
    assert!(
        first
            .content
            .contains(&format!("continue with offset={next}]"))
    );
    assert!(!first.omitted_headings.is_empty());
    assert_eq!(
        first.omitted_headings.len().min(12),
        first.omitted_headings.len()
    );
    let second = f
        .fetch_page(
            &url,
            FetchFormat::Markdown,
            PageOptions {
                max_chars: 1000,
                offset: next,
                compact: false,
            },
        )
        .await
        .expect("ok");
    assert!(
        second.content.starts_with("## Part") || second.content.starts_with("lorem ipsum"),
        "a window starts at a block: {}",
        second.content
    );
    assert_eq!(second.compacted_chars, 0);
    let past = f
        .fetch_page(
            &url,
            FetchFormat::Markdown,
            PageOptions {
                max_chars: 1000,
                offset: 1_000_000,
                compact: true,
            },
        )
        .await
        .expect("ok");
    assert!(
        past.content
            .starts_with("[offset 1000000 is past the end ("),
        "{}",
        past.content
    );
    assert!(!past.truncated);
}
