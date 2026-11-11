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
        let err = check_url(s, false).await.expect_err(s);
        assert!(matches!(err, FetchError::Rejected(_)), "{s}: {err}");
    }
    assert!(
        check_url("https://user:pass@example.com/a", false)
            .await
            .unwrap_err()
            .to_string()
            .contains("credentials")
    );
}

#[tokio::test]
async fn allows_public_ip_literal() {
    check_url("http://93.184.216.34/", false)
        .await
        .expect("public ip");
    assert!(!is_blocked_hostname("example.com", false));
    assert!(!is_blocked_ip("8.8.8.8".parse().unwrap(), false));
    assert!(is_blocked_ip("169.254.169.254".parse().unwrap(), false));
    assert!(is_blocked_ip("100.64.1.1".parse().unwrap(), false));
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
        assert!(is_blocked_hostname(h, false), "{h}");
    }
    for h in [
        "corporate.example.com",
        "lanstuff.example.com",
        "internal-tools.example.org",
        "example.corp.com",
    ] {
        assert!(!is_blocked_hostname(h, false), "{h}");
    }
    // NAT64 carries the embedded IPv4: loopback refused, a public address not.
    assert!(is_blocked_ip("64:ff9b::7f00:1".parse().unwrap(), false));
    assert!(
        is_blocked_ip("64:ff9b::a00:1".parse().unwrap(), false),
        "10.0.0.1"
    );
    assert!(
        !is_blocked_ip("64:ff9b::808:808".parse().unwrap(), false),
        "8.8.8.8"
    );
    // 183 Z3-R3: 6to4 and Teredo tunnel to an IPv4 address: refused; other 2001:: stays open.
    assert!(
        is_blocked_ip("2002:7f00:1::1".parse().unwrap(), false),
        "6to4 of 127.0.0.1"
    );
    assert!(
        is_blocked_ip("2001:0:4136:e378::1".parse().unwrap(), false),
        "Teredo"
    );
    assert!(!is_blocked_ip(
        "2001:4860:4860::8888".parse().unwrap(),
        false
    ));
    // 240.0.0.0/4 reserved.
    assert!(is_blocked_ip("240.0.0.1".parse().unwrap(), false));
    assert!(is_blocked_ip("250.1.2.3".parse().unwrap(), false));
    assert!(
        !is_blocked_ip("223.255.255.254".parse().unwrap(), false),
        "just below 240/4"
    );
    for s in [
        "http://[64:ff9b::7f00:1]/",
        "http://240.0.0.1/",
        "http://git.internal/",
        "http://nas.home.arpa/",
    ] {
        let err = check_url(s, false).await.expect_err(s);
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

/// 16 §5 (2026-11-11): `private` admits the local network (literals and names); cloud
/// metadata, unspecified, broadcast, multicast, the reserved ranges, other schemes and
/// credentials stay refused.
#[tokio::test]
async fn private_admits_the_local_network_never_metadata() {
    for ip in [
        "127.0.0.1",
        "::1",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.8.123",
        "100.64.0.1",
        "169.254.1.1",
        "fc00::1",
        "fe80::1",
        "::ffff:192.168.1.1",
    ] {
        let ip = ip.parse().unwrap();
        assert!(!is_blocked_ip(ip, true), "{ip} on");
        assert!(is_blocked_ip(ip, false), "{ip} off");
    }
    for ip in [
        "169.254.169.254",
        "169.254.170.2",
        "100.100.100.200",
        "fd00:ec2::254",
        "::ffff:169.254.169.254",
        "64:ff9b::a9fe:a9fe",
        "0.0.0.0",
        "::",
        "255.255.255.255",
        "224.0.0.1",
        "ff02::1",
        "240.0.0.1",
        "2002:7f00:1::1",
    ] {
        assert!(is_blocked_ip(ip.parse().unwrap(), true), "{ip}");
    }
    for h in [
        "localhost",
        "Foo.Localhost",
        "service.local",
        "printer.lan",
        "nas.home.arpa",
        "git.internal",
    ] {
        assert!(!is_blocked_hostname(h, true), "{h}");
    }
    for h in [
        "metadata.google.internal",
        "metadata",
        "kubernetes.default",
        "kubernetes.default.svc.cluster.local",
    ] {
        assert!(is_blocked_hostname(h, true), "{h}");
    }
    for s in [
        "http://192.168.8.123:28499/v1/chat/completions",
        "http://[fe80::1]/",
    ] {
        check_url(s, true).await.expect(s);
    }
    for s in [
        "http://169.254.169.254/latest/meta-data",
        "http://[fd00:ec2::254]/",
        "http://metadata.google.internal/",
        "http://0.0.0.0/",
        "https://user:pass@127.0.0.1/",
        "file:///etc/passwd",
    ] {
        let err = check_url(s, true).await.expect_err(s);
        assert!(matches!(err, FetchError::Rejected(_)), "{s}: {err}");
    }
}

/// A name whose DNS answer is loopback passes the resolve check only under `private`.
#[tokio::test]
async fn private_dns_answer_passes_only_with_the_flag() {
    check_url("http://localhost:1/", true)
        .await
        .expect("localhost on");
    let err = check_url("http://localhost:1/", false).await.unwrap_err();
    assert!(err.to_string().contains("not allowed"), "{err}");
}

/// The fetcher's twin: an owner's scope reaches a loopback server by literal, and follows
/// its redirect to a loopback *name* (static check, then the guarded resolver); the
/// metadata endpoint stays refused; a contact's scope and an unscoped call get today's
/// refusal on the first hop. No test allowlist anywhere.
#[tokio::test]
async fn lan_twin_serves_owner_scopes_only() {
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("http://localhost:{port}/b")),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<p>on the lan</p>", "text/html"))
        .mount(&server)
        .await;
    let fetcher = Fetcher::new("theuth-test", 2_097_152, 5)
        .and_then(|f| f.with_private_network(true))
        .expect("client");
    let start = format!("{}/a", server.uri());
    let r = tools::egress::scope(true, fetcher.fetch(&start, FetchFormat::Text, 4000))
        .await
        .expect("owner reaches loopback");
    assert!(r.content.contains("on the lan"), "{}", r.content);
    assert!(r.url.ends_with("/b"), "{}", r.url);
    for s in [
        "http://169.254.169.254/latest/meta-data",
        "http://metadata.google.internal/",
    ] {
        let err = tools::egress::scope(true, fetcher.fetch(s, FetchFormat::Text, 4000))
            .await
            .expect_err(s);
        assert!(matches!(err, FetchError::Rejected(_)), "{s}: {err}");
    }
    let refused = format!("IP address `{}` is not allowed", server.address().ip());
    for err in [
        tools::egress::scope(false, fetcher.fetch(&start, FetchFormat::Text, 4000))
            .await
            .unwrap_err(),
        fetcher
            .fetch(&start, FetchFormat::Text, 4000)
            .await
            .unwrap_err(),
    ] {
        assert!(err.to_string().contains(&refused), "{err}");
    }
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
    // 122 D6: the window is at most 30 chars; the marker after it does not count.
    let (window, marker) = resp.content.split_once("\n\n[… ").expect("marker");
    assert!(window.chars().count() <= 30, "{window:?}");
    assert!(
        marker.ends_with(&format!(
            "continue with offset={}]",
            resp.next_offset.unwrap()
        )),
        "{marker}"
    );
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
