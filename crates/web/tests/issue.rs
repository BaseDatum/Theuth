//! Design doc 122 D1 + R1-D2: every marker row, the benign cases, the `next`
//! table, `Retry-After` forms, and a 403 wall through the fetcher (wiremock).

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use url::Url;
use web::fetch::issue::{CHARS_PER_WORD, IssueKind, Next, PageIssue, detect, retry_after};
use web::fetch::{FetchFormat, Fetcher};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn url() -> Url {
    Url::parse("https://example.test/page").unwrap()
}

fn page(title: &str, body: &str) -> String {
    format!("<!doctype html><html><head><title>{title}</title></head><body>{body}</body></html>")
}

/// `n` words of prose as paragraphs (6 chars per word incl. the space).
fn prose(words: usize) -> String {
    (0..words)
        .map(|i| if i % 40 == 39 { "words\n\n" } else { "words " })
        .collect()
}

fn pad(bytes: usize) -> String {
    format!("<!-- {} -->", "x".repeat(bytes))
}

fn d(status: u16, body: &str, prose_chars: usize) -> Option<PageIssue> {
    detect(status, &HeaderMap::new(), &url(), body, prose_chars)
}

fn kind(i: &Option<PageIssue>) -> Option<IssueKind> {
    i.as_ref().map(|i| i.kind)
}

#[test]
fn chars_per_word_is_pinned() {
    assert_eq!(CHARS_PER_WORD, 6);
}

#[test]
fn row_1_cf_mitigated_header_is_conclusive() {
    let mut h = HeaderMap::new();
    h.insert("cf-mitigated", HeaderValue::from_static("challenge"));
    let i = detect(200, &h, &url(), &page("Fine", &prose(2000)), 12_000).unwrap();
    assert_eq!(i.kind, IssueKind::AccessChallenge);
    assert_eq!(i.detail, "cloudflare");
    let i = detect(403, &h, &url(), "", 0).unwrap();
    assert_eq!(i.detail, "HTTP 403 · cloudflare");
    assert_eq!(
        i.next,
        vec![Next::Rendered, Next::ImportCookies, Next::Handoff]
    );
}

#[test]
fn row_2_orchestrate_paths_not_the_loose_substring() {
    for marker in [
        "window._cf_chl_opt={}",
        "/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1",
        "/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1",
    ] {
        let i = d(
            403,
            &page(
                "Just a moment…",
                &format!("<script src=\"{marker}\"></script>"),
            ),
            0,
        );
        assert_eq!(i.unwrap().detail, "HTTP 403 · cloudflare", "{marker}");
    }
    // github.com-style benign beacon: the loose substring alone is not an issue.
    let benign = page(
        "Repo · GitHub",
        &format!(
            "<script src=\"/cdn-cgi/challenge-platform/scripts/jsd/main.js\"></script>{}",
            prose(500)
        ),
    );
    assert_eq!(d(200, &benign, 3000), None);
}

#[test]
fn row_3_turnstile_is_size_gated() {
    let widget = r#"<div class="cf-turnstile" data-sitekey="x"></div>"#;
    let small = page("Verify", widget);
    assert_eq!(d(200, &small, 0).unwrap().detail, "cloudflare:turnstile");
    // A > 100 KB article embedding Turnstile in its comment form is content.
    let article = page(
        "A long read",
        &format!("{}{widget}{}", prose(1000), pad(110_000)),
    );
    assert!(article.len() > 100_000);
    assert_eq!(d(200, &article, 6000), None);
}

#[test]
fn rows_4_to_7_vendor_markers() {
    let h = page(
        "Verify",
        r#"<script src="https://js.hcaptcha.com/1/api.js"></script><div class="h-captcha"></div>"#,
    );
    assert_eq!(d(200, &h, 0).unwrap().detail, "hcaptcha");
    // Over 50 KB the vendor row is gated; what remains is a big script shell (row 15), not a wall.
    assert_eq!(
        kind(&d(200, &format!("{h}{}", pad(50_000)), 0)),
        Some(IssueKind::EmptyShell),
        "hcaptcha size gate"
    );

    assert_eq!(
        d(405, &page("x", "<div id=\"awswaf-captcha\"></div>"), 0)
            .unwrap()
            .detail,
        "HTTP 405 · aws-waf"
    );
    assert_eq!(
        d(
            200,
            &page("x", "<script src=\"aws-waf-client-browser.js\"></script>"),
            0
        )
        .unwrap()
        .detail,
        "aws-waf"
    );
    let interstitial = page(
        "x",
        "<div class=\"interstitial-spinner\"></div><p>Verifying your connection</p>",
    );
    assert_eq!(d(200, &interstitial, 0).unwrap().detail, "aws-waf");
    assert_eq!(
        d(200, &format!("{interstitial}{}", pad(10_000)), 0),
        None,
        "interstitial size gate"
    );

    assert_eq!(
        d(
            403,
            &page(
                "x",
                "<script src=\"https://geo.captcha-delivery.com/captcha/\"></script>"
            ),
            0
        )
        .unwrap()
        .detail,
        "HTTP 403 · datadome"
    );

    assert_eq!(
        d(200, &page("Challenge Page", "<p>x</p>"), 0)
            .unwrap()
            .detail,
        "akamai"
    );
    let akamai = page("x", "<script>bazadebezolkohpepadr=1</script>");
    assert_eq!(d(403, &akamai, 0).unwrap().detail, "HTTP 403 · akamai");
    assert_eq!(
        kind(&d(200, &format!("{akamai}{}", pad(5_000)), 0)),
        Some(IssueKind::EmptyShell),
        "akamai token size gate (a 5 KB script shell is row 15)"
    );
}

#[test]
fn rows_7a_to_7d_revision_1_vendors() {
    assert_eq!(
        d(403, &page("x", "<script>_pxhd='a'</script>"), 0)
            .unwrap()
            .detail,
        "HTTP 403 · perimeterx"
    );
    assert_eq!(
        d(200, &page("x", "<div id=\"px-captcha\"></div>"), 0)
            .unwrap()
            .detail,
        "perimeterx"
    );
    assert_eq!(
        d(
            200,
            &format!("{}{}", page("x", "perimeterx"), pad(50_000)),
            0
        ),
        None
    );

    assert_eq!(
        d(
            403,
            &page("x", "<script src=\"/_Incapsula_Resource?x\"></script>"),
            0
        )
        .unwrap()
        .detail,
        "HTTP 403 · imperva"
    );
    assert_eq!(
        d(200, &page("x", "visid_incap_123"), 0).unwrap().detail,
        "imperva"
    );

    let recaptcha = page(
        "Verify",
        r#"<script src="https://www.google.com/recaptcha/api.js"></script><div class="g-recaptcha"></div>"#,
    );
    assert_eq!(d(200, &recaptcha, 0).unwrap().detail, "recaptcha");
    // A 400-word contact form embedding reCAPTCHA is not an issue (prose gate).
    let form = page("Contact us", &format!("{}{recaptcha}", prose(400)));
    assert_eq!(d(200, &form, 400 * CHARS_PER_WORD), None);
    assert_eq!(
        kind(&d(200, &format!("{recaptcha}{}", pad(30_000)), 0)),
        Some(IssueKind::EmptyShell),
        "recaptcha size gate"
    );

    assert_eq!(
        d(429, &page("x", "<script src=\"/ips.js\"></script>"), 0)
            .unwrap()
            .detail,
        "HTTP 429 · kasada"
    );
    assert_eq!(
        d(200, &page("x", "kpsdk-config"), 0).unwrap().detail,
        "kasada"
    );
    assert_eq!(
        d(200, &format!("{}{}", page("x", "kpsdk"), pad(30_000)), 0),
        None
    );
}

#[test]
fn row_8_antibot_titles_gated_on_prose() {
    for t in [
        "Just a moment...",
        "Attention Required! | Cloudflare",
        "Access Denied",
        "Checking your browser",
        "Please Wait",
        "One more step",
        "Verify you are human",
        "Bot Verification",
        "Security Check",
        "DDoS protection by x",
    ] {
        let i = d(
            403,
            &page(t, "<p>Enable JavaScript and cookies to continue.</p>"),
            60,
        );
        assert_eq!(kind(&i), Some(IssueKind::AccessChallenge), "{t}");
    }
    // 1020 vocabulary from the cf-ray markup; plain antibot title otherwise.
    let blocked = page(
        "Attention Required! | Cloudflare",
        "<p>Sorry, you have been blocked</p><span>cf-ray: 1</span>",
    );
    assert_eq!(
        d(403, &blocked, 60).unwrap().detail,
        "HTTP 403 · cloudflare:1020"
    );
    assert_eq!(
        d(200, &page("Access Denied", "<p>x</p>"), 10)
            .unwrap()
            .detail,
        "antibot-title"
    );
    // 200 words of prose and the title is just a title.
    assert_eq!(
        d(
            200,
            &page("Please wait for the show", &prose(400)),
            200 * CHARS_PER_WORD
        ),
        None
    );
}

#[test]
fn rows_9_to_11_status_rows() {
    let mut h = HeaderMap::new();
    h.insert("retry-after", HeaderValue::from_static("120"));
    let i = detect(429, &h, &url(), "", 0).unwrap();
    assert_eq!(
        (i.kind, i.detail.as_str(), i.retry_after, i.next),
        (
            IssueKind::RateLimited,
            "HTTP 429",
            Some(Duration::from_secs(120)),
            vec![Next::Retry]
        )
    );

    let i = d(403, &page("Forbidden", "<p>nope</p>"), 5).unwrap();
    assert_eq!(
        (i.kind, i.detail.as_str()),
        (IssueKind::AccessChallenge, "HTTP 403")
    );
    assert_eq!(d(401, "", 0).unwrap().detail, "HTTP 401");
    // A 403 JSON API body is not a wall.
    assert_eq!(d(403, r#"{"error":"forbidden","code":403}"#, 0), None);

    let i = detect(503, &h, &url(), &page("Service Unavailable", "<p>x</p>"), 5).unwrap();
    assert_eq!(
        (i.kind, i.detail.as_str(), i.retry_after),
        (
            IssueKind::ErrorPage,
            "HTTP 503 · error-page",
            Some(Duration::from_secs(120))
        )
    );
    let i = d(
        200,
        &page(
            "Error",
            "<p>This is taking longer than usual.</p><a href=\"\">Refresh</a>",
        ),
        20,
    )
    .unwrap();
    assert_eq!(
        (i.kind, i.detail.as_str(), i.next),
        (IssueKind::ErrorPage, "error-page", vec![Next::Retry])
    );
    // 502 without an issue marker: not diagnosed (the fetcher keeps `HTTP 502`).
    assert_eq!(d(502, &page("Bad Gateway", "<p>x</p>"), 5), None);
}

#[test]
fn rows_12_to_13_consent() {
    for u in [
        "https://consent.example.test/?continue=x",
        "https://example.test/consent?x=1",
        "https://example.test/consent/choices",
        "https://example.test/collectConsent",
        "https://example.test/x/consentcheck",
        "https://example.test/cmp/choose",
        "https://guce.advertising.com/collectIdentifiers",
    ] {
        let i = detect(
            200,
            &HeaderMap::new(),
            &Url::parse(u).unwrap(),
            &page("Hi", &prose(500)),
            3000,
        )
        .unwrap();
        assert_eq!(
            (i.kind, i.detail.as_str()),
            (IssueKind::ConsentWall, "consent:url"),
            "{u}"
        );
        assert_eq!(i.next, vec![Next::Rendered, Next::ImportCookies]);
    }
    let wall = page(
        "We value your privacy",
        r#"<div id="onetrust-consent-sdk"><p>We and our partners use cookies.</p></div>"#,
    );
    assert_eq!(d(200, &wall, 40).unwrap().detail, "consent:onetrust");
    assert_eq!(
        d(200, &page("Before you continue to Search", "<p>x</p>"), 300)
            .unwrap()
            .detail,
        "consent:title"
    );
    // 51 words of prose: the title is no longer the whole page.
    assert_eq!(d(200, &page("Cookie consent", &prose(400)), 301), None);
}

#[test]
fn row_14_login_gated_on_prose() {
    let i = d(
        200,
        &page("Sign in · Example", "<form class=\"login-form\"></form>"),
        80 * CHARS_PER_WORD,
    )
    .unwrap();
    assert_eq!(
        (i.kind, i.detail.as_str(), i.next),
        (
            IssueKind::LoginRequired,
            "login",
            vec![Next::ImportCookies, Next::Handoff]
        )
    );
    assert_eq!(
        kind(&d(401, &page("Log in", ""), 0)),
        Some(IssueKind::AccessChallenge),
        "401 wins (row 10)"
    );
    assert_eq!(
        kind(&d(200, &page("Login", ""), 0)),
        Some(IssueKind::LoginRequired)
    );
    // A "Sign in" title over 400 words of prose: an article about signing in.
    assert_eq!(
        d(
            200,
            &page("Sign in with passkeys explained", &prose(400)),
            400 * CHARS_PER_WORD
        ),
        None
    );
}

#[test]
fn rows_15_to_16_empty_shell() {
    let shell = page(
        "App",
        &format!(
            "<div id=\"content\"></div><script>{}</script>",
            "x".repeat(6_000)
        ),
    );
    let i = d(200, &shell, 100).unwrap();
    assert_eq!(
        (i.kind, i.detail.as_str(), i.next),
        (IssueKind::EmptyShell, "empty-shell", vec![Next::Rendered])
    );
    // Under 5 KB: not a shell by row 15.
    assert_eq!(
        d(200, &page("App", "<div></div><script>x</script>"), 100),
        None
    );
    // Row 16: big SPA markup with little prose; single quotes normalised.
    let spa = page(
        "App",
        &format!(
            "<div id='__next'></div><style>{}</style>",
            "y".repeat(60_000)
        ),
    );
    assert_eq!(d(200, &spa, 4_000).unwrap().detail, "empty-shell");
    assert_eq!(d(200, &spa, 4_800), None, "800 words of prose is a page");
    let plain = page("Big", &format!("<p>{}</p>", "z".repeat(60_000)));
    assert_eq!(d(200, &plain, 100), None, "no SPA marker, no script");
}

#[test]
fn next_table_is_fixed() {
    assert_eq!(
        IssueKind::AccessChallenge.next(),
        vec![Next::Rendered, Next::ImportCookies, Next::Handoff]
    );
    assert_eq!(IssueKind::RateLimited.next(), vec![Next::Retry]);
    assert_eq!(
        IssueKind::LoginRequired.next(),
        vec![Next::ImportCookies, Next::Handoff]
    );
    assert_eq!(
        IssueKind::ConsentWall.next(),
        vec![Next::Rendered, Next::ImportCookies]
    );
    assert_eq!(IssueKind::EmptyShell.next(), vec![Next::Rendered]);
    assert_eq!(IssueKind::ErrorPage.next(), vec![Next::Retry]);
    assert_eq!(IssueKind::Unsupported.next(), Vec::<Next>::new());
}

#[test]
fn retry_after_seconds_date_and_junk() {
    let mut h = HeaderMap::new();
    h.insert("retry-after", HeaderValue::from_static("7"));
    assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
    let future = (chrono::Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
    h.insert("retry-after", HeaderValue::from_str(&future).unwrap());
    let d = retry_after(&h).expect("http-date");
    assert!((85..=90).contains(&d.as_secs()), "{d:?}");
    h.insert(
        "retry-after",
        HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
    );
    assert_eq!(retry_after(&h), None, "past date");
    h.insert("retry-after", HeaderValue::from_static("-5"));
    assert_eq!(retry_after(&h), None);
    h.insert("retry-after", HeaderValue::from_static("soon"));
    assert_eq!(retry_after(&h), None);
    assert_eq!(retry_after(&HeaderMap::new()), None);
}

#[tokio::test]
async fn forbidden_with_cf_mitigated_through_the_fetcher() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("cf-mitigated", "challenge")
                .set_body_raw(
                    page(
                        "Just a moment…",
                        "<p>Enable JavaScript and cookies to continue.</p>",
                    )
                    .into_bytes(),
                    "text/html; charset=utf-8",
                ),
        )
        .mount(&server)
        .await;
    let fetcher = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()]);
    let r = fetcher
        .fetch(
            &format!("{}/catalog", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .expect("a diagnosed wall is Ok");
    let issue = r.issue.as_ref().expect("issue");
    assert_eq!(issue.kind, IssueKind::AccessChallenge);
    assert_eq!(issue.detail, "HTTP 403 · cloudflare");
    assert_eq!(r.title.as_deref(), Some("Just a moment…"));
    assert!(r.content.contains("Enable JavaScript"), "{}", r.content);
    let text = web::render::fetch_text(&r);
    assert!(
        text.starts_with("ISSUE: access_challenge (HTTP 403 · cloudflare) — next: rendered: true | browser_import_cookies | desktop_handoff"),
        "{text}"
    );
    // A 403 JSON body stays an error, as before.
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_raw(br#"{"error":"forbidden"}"#.to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    let err = fetcher
        .fetch(
            &format!("{}/api", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "HTTP 403");
}
