//! SSRF deny matrix and fetch behaviour (design doc 16 §5). Deny paths need
//! no network: every rejection here happens before DNS. Happy paths use
//! wiremock on loopback, allowlisted by exact `ip:port` only, so a redirect
//! to `127.0.0.1:80` stays blocked.

use web::fetch::{FetchError, FetchFormat, Fetcher, check_url, is_blocked_hostname, is_blocked_ip};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HTML: &str = include_str!("fixtures/fetch_sample.html");

fn fetcher(server: &MockServer) -> Fetcher {
    Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()])
}

#[tokio::test]
async fn rejects_ip_literals_localhost_credentials_and_schemes() {
    for s in [
        "http://127.0.0.1/",
        "http://[::1]/",
        "http://192.168.0.1/",
        "http://10.1.2.3/",
        "http://172.16.0.1/",
        "http://169.254.169.254/latest/meta-data",
        "http://100.64.0.1/",
        "http://[fc00::1]/",
        "http://[fe80::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://localhost:1/",
        "http://Foo.Localhost/",
        "http://service.local/",
        "http://metadata.google.internal/",
        "https://user:pass@example.com/a",
        "file:///etc/passwd",
        "ftp://example.com/",
        "data:text/plain,x",
        "/relative",
    ] {
        let err = check_url(s).await.expect_err(s);
        assert!(matches!(err, FetchError::Rejected(_)), "{s}: {err}");
    }
    assert!(
        check_url("https://user:pass@example.com/a")
            .await
            .unwrap_err()
            .to_string()
            .contains("credentials")
    );
}

#[tokio::test]
async fn allows_public_ip_literal() {
    check_url("http://93.184.216.34/").await.expect("public ip");
    assert!(!is_blocked_hostname("example.com"));
    assert!(!is_blocked_ip("8.8.8.8".parse().unwrap()));
    assert!(is_blocked_ip("169.254.169.254".parse().unwrap()));
    assert!(is_blocked_ip("100.64.1.1".parse().unwrap()));
}

#[tokio::test]
async fn internal_suffixes_nat64_and_reserved_v4_are_refused() {
    for h in [
        "intranet.corp",
        "corp",
        "git.internal",
        "internal",
        "printer.lan",
        "lan",
        "nas.home.arpa",
        "home.arpa",
        "wiki.intranet",
        "intranet",
        "Build.CORP.",
    ] {
        assert!(is_blocked_hostname(h), "{h}");
    }
    for h in [
        "corporate.example.com",
        "lanstuff.example.com",
        "internal-tools.example.org",
        "example.corp.com",
    ] {
        assert!(!is_blocked_hostname(h), "{h}");
    }
    // NAT64 carries the embedded IPv4: loopback refused, a public address not.
    assert!(is_blocked_ip("64:ff9b::7f00:1".parse().unwrap()));
    assert!(is_blocked_ip("64:ff9b::a00:1".parse().unwrap()), "10.0.0.1");
    assert!(
        !is_blocked_ip("64:ff9b::808:808".parse().unwrap()),
        "8.8.8.8"
    );
    // 240.0.0.0/4 reserved.
    assert!(is_blocked_ip("240.0.0.1".parse().unwrap()));
    assert!(is_blocked_ip("250.1.2.3".parse().unwrap()));
    assert!(
        !is_blocked_ip("223.255.255.254".parse().unwrap()),
        "just below 240/4"
    );
    for s in [
        "http://[64:ff9b::7f00:1]/",
        "http://240.0.0.1/",
        "http://git.internal/",
        "http://nas.home.arpa/",
    ] {
        let err = check_url(s).await.expect_err(s);
        assert!(matches!(err, FetchError::Rejected(_)), "{s}: {err}");
    }
}

#[tokio::test]
async fn fetcher_rejects_loopback_and_metadata_without_allowlist() {
    let fetcher = Fetcher::new("theuth-test", 2_097_152, 5).expect("client");
    for s in [
        "http://169.254.169.254/latest/meta-data",
        "http://localhost:1/",
        "http://127.0.0.1/",
    ] {
        let err = fetcher
            .fetch(s, FetchFormat::Markdown, 4000)
            .await
            .expect_err(s);
        assert!(matches!(err, FetchError::Rejected(_)), "{s}: {err}");
    }
}

#[tokio::test]
async fn redirect_to_private_is_rejected_on_the_hop() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pub"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "http://127.0.0.1/x"))
        .mount(&server)
        .await;
    let err = fetcher(&server)
        .fetch(
            &format!("{}/pub", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Rejected(_)), "got {err}");
}

#[tokio::test]
async fn html_page_as_markdown_and_text() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/doc"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(HTML.as_bytes(), "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;
    let fetcher = fetcher(&server);
    let url = format!("{}/doc", server.uri());

    let md = fetcher
        .fetch(&url, FetchFormat::Markdown, 4000)
        .await
        .expect("markdown");
    assert_eq!(md.title.as_deref(), Some("Sample Article Title"));
    assert!(md.content.contains("# Hello fetch world"), "{}", md.content);
    assert!(
        md.content.contains("[the docs](https://example.com/docs)"),
        "{}",
        md.content
    );
    for noise in ["alert(", "Copyright", "Home", "Sidebar", "Enable JS"] {
        assert!(
            !md.content.contains(noise),
            "{noise} leaked: {}",
            md.content
        );
    }
    assert!(!md.truncated);

    let text = fetcher
        .fetch(&url, FetchFormat::Text, 4000)
        .await
        .expect("text");
    assert!(text.content.contains("Hello fetch world"));
    assert!(text.content.contains("the docs"));
    assert!(
        !text.content.contains("](") && !text.content.contains("# "),
        "{}",
        text.content
    );
}

#[tokio::test]
async fn max_chars_truncation_sets_flag() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/small"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(HTML.as_bytes(), "text/html"))
        .mount(&server)
        .await;
    let resp = fetcher(&server)
        .fetch(&format!("{}/small", server.uri()), FetchFormat::Text, 30)
        .await
        .expect("ok");
    assert!(resp.truncated);
    assert_eq!(resp.content.chars().count(), 30);
    assert!(resp.content.ends_with('…'));
}

#[tokio::test]
async fn plain_text_and_binary_bodies() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/plain"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(b"  hello <b>not html</b>  ".to_vec(), "text/plain"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bin"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 16], "application/zip"))
        .mount(&server)
        .await;
    // A PDF is converted, not rejected; a broken one fails with the converter's reason.
    Mock::given(method("GET"))
        .and(path("/doc.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(b"%PDF-1.7\n%garbage".to_vec(), "application/pdf"),
        )
        .mount(&server)
        .await;
    let fetcher = fetcher(&server);
    let plain = fetcher
        .fetch(
            &format!("{}/plain", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .expect("plain");
    assert_eq!(plain.content, "hello <b>not html</b>");
    assert_eq!(plain.title, None);
    let err = fetcher
        .fetch(
            &format!("{}/bin", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Failed(_)), "{err}");
    let err = fetcher
        .fetch(
            &format!("{}/doc.pdf", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, FetchError::Failed(m) if m.starts_with("Pdf:")),
        "{err}"
    );
}
