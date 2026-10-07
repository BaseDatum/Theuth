//! `Fetcher::request` (design doc 44 §0.6): arbitrary method + headers + body
//! behind the same guard as `fetch`, non-2xx returned as `Ok`, redirects
//! rewritten per RFC 9110 (301/302/303 → GET, 307/308 keep method and body),
//! credential headers never forwarded to another origin (46 §5).
//! Happy paths use wiremock on loopback, allowlisted by exact `ip:port`.

use web::fetch::{FetchError, Fetcher};
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fetcher(server: &MockServer, max_bytes: usize) -> Fetcher {
    Fetcher::new("theuth-test", max_bytes, 5)
        .expect("client")
        .allowing(vec![*server.address()])
}

fn hdr(name: &str, value: &str) -> (String, String) {
    (name.to_owned(), value.to_owned())
}

#[tokio::test]
async fn post_with_header_and_body_reaches_the_server() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("x-csrf-token", "abc"))
        .and(body_string(r#"{"query":"{ me }"}"#))
        .respond_with(ResponseTemplate::new(201).set_body_raw(r#"{"data":{}}"#, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    let r = fetcher(&server, 1 << 20)
        .request(
            "POST",
            &format!("{}/graphql", server.uri()),
            &[
                hdr("x-csrf-token", "abc"),
                hdr("Host", "evil.example"),
                hdr("Content-Length", "1"),
            ],
            Some(br#"{"query":"{ me }"}"#.to_vec()),
        )
        .await
        .expect("ok");
    assert_eq!(r.status, 201);
    assert_eq!(r.body, br#"{"data":{}}"#);
    assert!(!r.truncated);
    assert!(
        r.headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"),
        "{:?}",
        r.headers
    );
}

#[tokio::test]
async fn non_2xx_is_ok_with_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("nope"))
        .mount(&server)
        .await;
    let r = fetcher(&server, 1 << 20)
        .request("GET", &format!("{}/missing", server.uri()), &[], None)
        .await
        .expect("ok");
    assert_eq!(r.status, 404);
    assert_eq!(r.body, b"nope");
}

#[tokio::test]
async fn found_redirect_from_post_is_followed_as_get() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/b"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string("landed"))
        .expect(1)
        .mount(&server)
        .await;
    let r = fetcher(&server, 1 << 20)
        .request(
            "POST",
            &format!("{}/a", server.uri()),
            &[],
            Some(b"payload".to_vec()),
        )
        .await
        .expect("ok");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"landed");
    assert_eq!(r.url, format!("{}/b", server.uri()));
}

#[tokio::test]
async fn temporary_redirect_keeps_post_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(307).insert_header("Location", "/b"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/b"))
        .and(body_string("payload"))
        .respond_with(ResponseTemplate::new(200).set_body_string("kept"))
        .expect(1)
        .mount(&server)
        .await;
    let r = fetcher(&server, 1 << 20)
        .request(
            "POST",
            &format!("{}/a", server.uri()),
            &[],
            Some(b"payload".to_vec()),
        )
        .await
        .expect("ok");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"kept");
}

#[tokio::test]
async fn cross_origin_redirect_with_caller_headers_is_rejected() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/b", b.uri())),
        )
        .mount(&a)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string("leaked"))
        .expect(1)
        .mount(&b)
        .await;
    let fetcher = Fetcher::new("theuth-test", 1 << 20, 5)
        .expect("client")
        .allowing(vec![*a.address(), *b.address()]);
    for name in ["Authorization", "cookie", "X-Api-Key", "x-custom-token"] {
        let err = fetcher
            .request(
                "GET",
                &format!("{}/a", a.uri()),
                &[hdr(name, "secret")],
                None,
            )
            .await
            .expect_err("header forwarding");
        assert!(matches!(err, FetchError::Rejected(_)), "{name}: got {err}");
    }
    // The same hop without caller headers is an ordinary redirect.
    let r = fetcher
        .request("GET", &format!("{}/a", a.uri()), &[], None)
        .await
        .expect("ok");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"leaked");
}

#[tokio::test]
async fn same_origin_redirect_keeps_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/b"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .and(header("authorization", "Bearer t"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;
    let r = fetcher(&server, 1 << 20)
        .request(
            "GET",
            &format!("{}/a", server.uri()),
            &[hdr("authorization", "Bearer t")],
            None,
        )
        .await
        .expect("ok");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"ok");
}

/// 108 D1: a conditional GET's `304 Not Modified` has no `Location` and is
/// not a redirect — it comes back as a response with its status and an empty
/// body, so a sensor's `http_request` sees `status: 304`. A 301 without
/// `Location` is still the fetcher's error.
#[tokio::test]
async fn not_modified_is_a_response_not_a_redirect() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304).insert_header("ETag", "\"v1\""))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/moved"))
        .respond_with(ResponseTemplate::new(301))
        .mount(&server)
        .await;
    let f = fetcher(&server, 1 << 20);
    let r = f
        .request(
            "GET",
            &format!("{}/feed", server.uri()),
            &[hdr("if-none-match", "\"v1\"")],
            None,
        )
        .await
        .expect("304 is Ok");
    assert_eq!(r.status, 304);
    assert!(r.body.is_empty());
    assert_eq!(r.url, format!("{}/feed", server.uri()));
    assert!(r.headers.iter().any(|(k, v)| k == "etag" && v == "\"v1\""));
    let e = f
        .request("GET", &format!("{}/moved", server.uri()), &[], None)
        .await
        .expect_err("a 301 without Location is still an error");
    assert!(
        matches!(&e, FetchError::Failed(m) if m.contains("missing Location")),
        "{e}"
    );
}

/// wiremock always sends `Content-Length`, which the fetcher refuses up front;
/// the streaming cap needs a chunked body, so serve one from a std socket.
fn chunked_server(total: usize) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut head = [0u8; 4096];
        let _ = sock.read(&mut head);
        let mut out =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        out.extend_from_slice(format!("{total:x}\r\n").as_bytes());
        out.extend(std::iter::repeat_n(b'x', total));
        out.extend_from_slice(b"\r\n0\r\n\r\n");
        sock.write_all(&out).expect("write");
    });
    addr
}

#[tokio::test]
async fn oversize_chunked_body_is_cut_and_flagged() {
    let addr = chunked_server(4096);
    let r = Fetcher::new("theuth-test", 1024, 5)
        .expect("client")
        .allowing(vec![addr])
        .request("GET", &format!("http://{addr}/big"), &[], None)
        .await
        .expect("ok");
    assert!(r.truncated);
    assert!(r.body.len() <= 1024, "{}", r.body.len());
}

#[tokio::test]
async fn oversize_content_length_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(4096)))
        .mount(&server)
        .await;
    let err = fetcher(&server, 1024)
        .request("GET", &format!("{}/big", server.uri()), &[], None)
        .await
        .expect_err("refused");
    assert!(matches!(err, FetchError::Failed(_)), "got {err}");
}

#[tokio::test]
async fn loopback_without_allowlist_is_rejected() {
    let server = MockServer::start().await;
    let fetcher = Fetcher::new("theuth-test", 1 << 20, 5).expect("client");
    let err = fetcher
        .request("GET", &server.uri(), &[], None)
        .await
        .expect_err("guard");
    assert!(matches!(err, FetchError::Rejected(_)), "got {err}");
}
