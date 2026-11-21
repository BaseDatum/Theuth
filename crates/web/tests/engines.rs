//! Offline wire-contract tests for the built-in engines (design doc 16 §3.2),
//! one wiremock server per test; fixtures under `tests/fixtures/` are dated
//! captures of the real response shapes.

use std::collections::HashMap;

use web::RawResult;
use web::engines::{self, Arxiv, Bing, Brave, DuckDuckGo, Github, Google, Qwant, Wikipedia};
use web::{Category, FeatureKind, Provider, ProviderError, Query, Recency};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DDG_HTML: &str = include_str!("fixtures/ddg_html.html");
const DDG_IA: &str = include_str!("fixtures/ddg_instant_answer.json");
const WIKI_SEARCH: &str = include_str!("fixtures/wikipedia_search.json");
const WIKI_SUMMARY: &str = include_str!("fixtures/wikipedia_summary.json");
const ARXIV: &str = include_str!("fixtures/arxiv_atom.xml");
const GITHUB: &str = include_str!("fixtures/github_repos.json");

/// Live captures (16 §4.1), dated in their leading comment / sibling `.txt`.
/// Google and Qwant answered this network with their bot walls and Bing with
/// a decoy SERP, so those three fixtures drive the `Blocked` paths and their
/// happy paths stay inline; Brave is the one real organic capture.
const BING_DECOY: &str = include_str!("fixtures/bing_search-decoy-2026-09-17.html");
const BRAVE_SERP: &str = include_str!("fixtures/brave_search-2026-09-17.html");
const GOOGLE_SORRY: &str = include_str!("fixtures/google_wml-blocked-2026-09-17.html");
const QWANT_CAPTCHA: &str = include_str!("fixtures/qwant_web-blocked-2026-09-17.json");

fn query(text: &str) -> Query {
    Query {
        text: text.to_owned(),
        category: Category::General,
        limit: 8,
        site: None,
        recent: None,
    }
}

fn ok(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(body)
}

async fn header_of(server: &MockServer, index: usize, name: &str) -> String {
    let requests = server.received_requests().await.expect("recording on");
    requests[index]
        .headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// At least `min` hits with a snippet; every hit has a non-empty title and
/// an absolute `https` URL that is not a SERP redirector. (Real SERPs carry
/// the odd snippet-less row, e.g. Brave's Reddit cards.)
fn assert_clean_hits(hits: &[RawResult], min: usize) {
    let with_snippet = hits.iter().filter(|h| !h.snippet.is_empty()).count();
    assert!(
        with_snippet >= min,
        "expected >= {min} hits with snippets, got {with_snippet} of {}",
        hits.len()
    );
    for h in hits {
        assert!(!h.title.is_empty(), "{h:?}");
        assert!(h.url.starts_with("https://"), "{h:?}");
        assert!(!h.url.contains("bing.com/ck/a"), "{h:?}");
        assert!(!h.url.contains("/url?q="), "{h:?}");
    }
}

#[test]
fn builtin_constructs_every_default_engine() {
    let none = HashMap::new();
    assert_eq!(
        engines::BUILTIN,
        &[
            "google",
            "bing",
            "brave",
            "qwant",
            "duckduckgo",
            "wikipedia",
            "arxiv",
            "github"
        ]
    );
    for name in engines::BUILTIN {
        let provider = engines::builtin(name, &none).expect(name);
        assert_eq!(provider.meta().name, *name);
    }
    let ddg = engines::builtin("duckduckgo", &none).expect("ddg");
    assert_eq!(
        ddg.meta().features,
        &[
            FeatureKind::Answer,
            FeatureKind::KnowledgePanel,
            FeatureKind::Definition,
            FeatureKind::Related,
        ]
    );
}

#[tokio::test]
async fn ddg_posts_form_with_browser_ua_and_parses_the_serp() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .and(body_string_contains(
            "q=site%3Arust-lang.org+rust+async+trait",
        ))
        .and(body_string_contains("df=w"))
        .respond_with(ok(DDG_HTML))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ok("{}"))
        .mount(&server)
        .await;

    let mut q = query("rust async trait");
    q.site = Some("rust-lang.org".to_owned());
    q.recent = Some(Recency::Week);
    let (hits, features) = DuckDuckGo::at(&server.uri())
        .search(&reqwest::Client::new(), &q)
        .await
        .expect("serp");

    assert!(features.is_empty());
    let urls: Vec<&str> = hits.iter().map(|h| h.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://rust-lang.github.io/async-fundamentals-initiative/evaluation/survey.html",
            "https://crates.io/crates/async-trait",
            "https://rust-lang.github.io/rfcs/3185-static-async-fn-in-trait.html",
        ],
        "ad row skipped, redirect wrapper unwrapped, duplicate dropped"
    );
    assert_eq!(hits[0].title, "Async fn in traits");
    assert_eq!(
        hits[0].snippet,
        "Async functions in traits are a long-awaited feature for the Rust language."
    );
    assert_eq!(
        hits[2].snippet,
        "Static async fn in traits. This RFC proposes a path for async functions in traits."
    );

    let ua = header_of(&server, 0, "user-agent").await;
    assert!(ua.contains("Mozilla/"), "browser-like UA required: {ua}");
}

#[tokio::test]
async fn ddg_limit_truncates_and_empty_serp_is_ok() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ok(DDG_HTML))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ok("{}"))
        .mount(&server)
        .await;
    let ddg = DuckDuckGo::at(&server.uri());
    let client = reqwest::Client::new();

    let mut q = query("rust");
    q.limit = 1;
    let (hits, _) = ddg.search(&client, &q).await.expect("serp");
    assert_eq!(hits.len(), 1);

    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ok(
            "<html><body><div class=\"results\"></div></body></html>",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ok("{}"))
        .mount(&server)
        .await;
    let (hits, _) = ddg
        .search(&client, &query("zzz"))
        .await
        .expect("empty serp");
    assert!(hits.is_empty());
}

#[tokio::test]
async fn ddg_403_falls_back_to_lite() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ResponseTemplate::new(403).set_body_string("email us"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lite/"))
        .and(query_param("q", "rust"))
        .respond_with(ok(concat!(
            "<table><tr><td>",
            "<a rel=\"nofollow\" href=\"//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Flite\" class='result-link'>Lite Hit</a>",
            "</td></tr><tr><td class='result-snippet'>from lite</td></tr></table>"
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ok("{}"))
        .mount(&server)
        .await;

    let (hits, _) = DuckDuckGo::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect("lite");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Lite Hit");
    assert_eq!(hits[0].url, "https://example.com/lite");
    assert_eq!(hits[0].snippet, "from lite");
}

#[tokio::test]
async fn ddg_blocked_on_both_hosts_is_blocked() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lite/"))
        .respond_with(ok("<html><body>Just a moment...</body></html>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ok("{}"))
        .mount(&server)
        .await;

    let err = DuckDuckGo::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect_err("blocked");
    assert!(matches!(err, ProviderError::Blocked(_)), "{err}");
}

#[tokio::test]
async fn ddg_instant_answers_become_features() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ok("<html></html>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", "rust programming language"))
        .and(query_param("format", "json"))
        .and(query_param("no_html", "1"))
        .respond_with(ok(DDG_IA))
        .expect(1)
        .mount(&server)
        .await;

    let (_, features) = DuckDuckGo::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust programming language"))
        .await
        .expect("ia");
    let kinds: Vec<FeatureKind> = features.iter().map(|f| f.kind).collect();
    assert_eq!(kinds, [FeatureKind::KnowledgePanel, FeatureKind::Related]);

    let panel = &features[0];
    assert_eq!(panel.provider, "duckduckgo");
    assert_eq!(panel.title.as_deref(), Some("Rust (programming language)"));
    assert!(
        panel
            .text
            .as_deref()
            .is_some_and(|t| t.starts_with("Rust is a general-purpose"))
    );
    assert_eq!(
        panel.image.as_deref(),
        Some("https://duckduckgo.com/i/2b9d6e3a.png")
    );
    assert_eq!(panel.links.len(), 1);
    assert_eq!(panel.links[0].title, "Wikipedia");
    assert_eq!(
        panel.attributes,
        [
            ("Designed by".to_owned(), "Graydon Hoare".to_owned()),
            ("Developer".to_owned(), "Rust Foundation".to_owned()),
            ("First appeared".to_owned(), "May 15, 2015".to_owned()),
        ],
        "structured infobox rows are skipped"
    );

    let related = &features[1];
    let titles: Vec<&str> = related.items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles.len(), 3, "Topics groups are flattened");
    assert!(titles[2].starts_with("Graydon Hoare"));
    assert_eq!(
        related.items[0].url.as_deref(),
        Some("https://duckduckgo.com/Rust_Foundation")
    );
}

#[test]
fn ddg_answer_and_definition_features() {
    let doc = serde_json::json!({
        "Heading": "", "Answer": "4294967296", "AnswerType": "calc",
        "Definition": "a systems programming language", "DefinitionSource": "Wiktionary",
        "DefinitionURL": "https://en.wiktionary.org/wiki/Rust", "AbstractText": "", "Infobox": "",
        "RelatedTopics": (0..20).map(|i| serde_json::json!({"Text": format!("t{i}"), "FirstURL": "https://duckduckgo.com/x"})).collect::<Vec<_>>()
    });
    let features = engines::duckduckgo::features(&doc);
    let kinds: Vec<FeatureKind> = features.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [
            FeatureKind::Answer,
            FeatureKind::Definition,
            FeatureKind::Related
        ]
    );
    assert_eq!(features[0].text.as_deref(), Some("4294967296"));
    assert_eq!(features[0].title, None, "empty heading is not a title");
    assert_eq!(
        features[1].links[0].url,
        "https://en.wiktionary.org/wiki/Rust"
    );
    assert_eq!(features[2].items.len(), 8, "related topics capped");
}

#[tokio::test]
async fn wikipedia_results_and_summary_panel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/w/api.php"))
        .and(query_param("action", "query"))
        .and(query_param("list", "search"))
        .and(query_param("srsearch", "rust programming language"))
        .and(query_param("srlimit", "8"))
        .and(header("user-agent", engines::user_agent()))
        .respond_with(ok(WIKI_SEARCH))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/rest_v1/page/summary/Rust_(programming_language)",
        ))
        .and(header("user-agent", engines::user_agent()))
        .respond_with(ok(WIKI_SUMMARY))
        .expect(1)
        .mount(&server)
        .await;

    let (hits, features) = Wikipedia::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust programming language"))
        .await
        .expect("wikipedia");

    assert_eq!(hits.len(), 5);
    assert_eq!(hits[0].title, "Rust (programming language)");
    assert!(
        hits[0].url.ends_with("/wiki/Rust_(programming_language)"),
        "{}",
        hits[0].url
    );
    assert!(
        hits[0]
            .snippet
            .starts_with("Rust is a general-purpose programming language")
    );
    assert!(!hits[0].snippet.contains('<'), "searchmatch spans stripped");
    assert!(hits.iter().all(|h| h.published.is_none()));

    assert_eq!(features.len(), 1);
    let panel = &features[0];
    assert_eq!(panel.kind, FeatureKind::KnowledgePanel);
    assert_eq!(panel.provider, "wikipedia");
    assert_eq!(panel.title.as_deref(), Some("Rust (programming language)"));
    assert!(
        panel
            .text
            .as_deref()
            .is_some_and(|t| t.contains("garbage collector"))
    );
    assert!(
        panel
            .image
            .as_deref()
            .is_some_and(|i| i.starts_with("https://upload.wikimedia.org/"))
    );
    assert_eq!(
        panel.attributes,
        [(
            "description".to_owned(),
            "General-purpose programming language".to_owned()
        )]
    );
    assert_eq!(
        panel.links[0].url,
        "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    );
}

#[tokio::test]
async fn wikipedia_summary_failure_keeps_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/w/api.php"))
        .respond_with(ok(WIKI_SEARCH))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/rest_v1/page/summary/Rust_(programming_language)",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let (hits, features) = Wikipedia::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect("wikipedia");
    assert_eq!(hits.len(), 5);
    assert!(features.is_empty());
}

#[tokio::test]
async fn arxiv_results_carry_published_dates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/query"))
        .and(query_param("search_query", "all:attention transformer"))
        .and(query_param("start", "0"))
        .and(query_param("max_results", "8"))
        .respond_with(ok(ARXIV))
        .expect(1)
        .mount(&server)
        .await;

    let (hits, features) = Arxiv::at(&server.uri())
        .search(&reqwest::Client::new(), &query("attention transformer"))
        .await
        .expect("arxiv");
    assert!(features.is_empty());
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].title, "Dilated Neighborhood Attention Transformer");
    assert_eq!(
        hits[0].url, "https://arxiv.org/abs/2209.15001v3",
        "http upgraded"
    );
    assert_eq!(hits[0].published.as_deref(), Some("2022-09-29T17:57:08Z"));
    assert!(
        hits[0]
            .snippet
            .starts_with("Transformers are quickly becoming")
    );
    assert_eq!(hits[2].published.as_deref(), Some("2023-09-04T16:09:28Z"));
}

#[tokio::test]
async fn github_results_carry_stars_and_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/repositories"))
        .and(query_param("q", "rust async trait"))
        .and(query_param("per_page", "8"))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("authorization", "Bearer ghp_test"))
        .respond_with(ok(GITHUB))
        .expect(1)
        .mount(&server)
        .await;

    let (hits, _) = Github::at(&server.uri(), Some("ghp_test".to_owned()))
        .search(&reqwest::Client::new(), &query("rust async trait"))
        .await
        .expect("github");
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].title, "dtolnay/async-trait");
    assert_eq!(hits[0].url, "https://github.com/dtolnay/async-trait");
    assert_eq!(hits[0].snippet, "Type erasure for async trait methods");
    assert_eq!(hits[0].score_hint, Some(9000.0));
    assert_eq!(hits[2].snippet, "", "null description");
}

#[tokio::test]
async fn github_blank_token_sends_no_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/repositories"))
        .respond_with(ok(GITHUB))
        .mount(&server)
        .await;
    Github::at(&server.uri(), Some("   ".to_owned()))
        .search(&reqwest::Client::new(), &query("x"))
        .await
        .expect("github");
    assert_eq!(header_of(&server, 0, "authorization").await, "");
}

#[tokio::test]
async fn github_403_with_rate_limit_body_is_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/repositories"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            r#"{"message":"API rate limit exceeded for 203.0.113.7.","documentation_url":"https://docs.github.com/rest/overview/rate-limits-for-the-rest-api"}"#,
        ))
        .mount(&server)
        .await;
    let err = Github::at(&server.uri(), None)
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect_err("rate limited");
    assert!(matches!(err, ProviderError::RateLimited), "{err}");

    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"message":"Bad credentials"}"#),
        )
        .mount(&server)
        .await;
    let err = Github::at(&server.uri(), Some("bad".to_owned()))
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect_err("unauthorized");
    assert!(matches!(err, ProviderError::Unauthorized), "{err}");
}

#[tokio::test]
async fn google_parses_wml_serp_with_nokia_ua_and_english_filter() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wml/search"))
        .and(query_param("q", "rust"))
        .and(query_param("lr", "lang_en"))
        .respond_with(ok(r#"<div class="zMzFAb">
  <a class="fuLhoc" href="https://www.rust-lang.org/">
    <span class="CVA68e">Rust</span>
  </a>
  <span class="FrIlee">A language.</span>
</div>
<table class="HExoMb"><tr><td><a class="ZWRArf" href="/search?q=rust+book">rust book</a></td></tr></table>"#))
        .expect(1)
        .mount(&server)
        .await;
    let (hits, features) = Google::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect("google");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Rust");
    assert_eq!(hits[0].url, "https://www.rust-lang.org/");
    assert_eq!(hits[0].snippet, "A language.");
    assert_eq!(features.len(), 1);
    assert_eq!(features[0].kind, FeatureKind::Related);
    assert_eq!(features[0].items[0].title, "rust book");
    let ua = header_of(&server, 0, "user-agent").await;
    assert!(
        engines::google::NOKIA_UAS.contains(&ua.as_str()),
        "Nokia UA required: {ua}"
    );
}

#[tokio::test]
async fn google_sorry_redirect_is_blocked() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wml/search"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "/sorry/index?continue=https://www.google.com/"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sorry/index"))
        .respond_with(ResponseTemplate::new(429).set_body_string(GOOGLE_SORRY))
        .expect(1)
        .mount(&server)
        .await;
    let err = Google::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect_err("sorry page");
    assert!(matches!(err, ProviderError::Blocked(_)), "{err}");
}

/// The captured Bing page is what a flagged client gets: HTTP 200, the right
/// `<title>`, ten `b_algo` rows for an unrelated query (som.today). The rows
/// parse and unwrap cleanly; `reject_decoys` then reports the page as blocked
/// instead of handing the model confident garbage.
#[tokio::test]
async fn bing_decoy_serp_is_blocked() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "rust programming"))
        .and(query_param("setlang", "en"))
        .respond_with(ok(BING_DECOY))
        .expect(1)
        .mount(&server)
        .await;
    let mut q = query("rust programming");
    q.limit = 20;
    let err = Bing::at(&server.uri())
        .search(&reqwest::Client::new(), &q)
        .await
        .expect_err("decoy page");
    assert!(
        matches!(&err, ProviderError::Blocked(m) if m.contains("decoy")),
        "{err}"
    );
    assert_eq!(
        header_of(&server, 0, "user-agent").await,
        engines::bing::BROWSER_UA
    );
}

#[tokio::test]
async fn bing_403_is_blocked_and_recency_is_unsupported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(403).set_body_string("<html>forbidden</html>"))
        .expect(1)
        .mount(&server)
        .await;
    let bing = Bing::at(&server.uri());
    let err = bing
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect_err("403");
    assert!(matches!(err, ProviderError::Blocked(_)), "{err}");

    let mut q = query("rust");
    q.recent = Some(Recency::Week);
    let err = bing
        .search(&reqwest::Client::new(), &q)
        .await
        .expect_err("no time filter");
    assert!(matches!(err, ProviderError::Unsupported), "{err}");
}

#[tokio::test]
async fn brave_fixture_yields_rows_dates_and_related() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "rust programming"))
        .and(query_param("source", "web"))
        .respond_with(ok(BRAVE_SERP))
        .expect(1)
        .mount(&server)
        .await;
    let mut q = query("rust programming");
    q.limit = 20;
    let (hits, features) = Brave::at(&server.uri())
        .search(&reqwest::Client::new(), &q)
        .await
        .expect("brave");
    assert_clean_hits(&hits, 3);
    let wiki = hits
        .iter()
        .find(|h| h.url == "https://en.wikipedia.org/wiki/Rust_(programming_language)")
        .expect("wikipedia row");
    assert_eq!(wiki.title, "Rust (programming language) - Wikipedia");
    assert_eq!(wiki.published.as_deref(), Some("3 weeks ago"));
    assert!(
        wiki.snippet
            .starts_with("Rust is a general-purpose programming language"),
        "date stripped from snippet: {}",
        wiki.snippet
    );
    assert_eq!(features.len(), 1);
    assert_eq!(features[0].kind, FeatureKind::Related);
    assert_eq!(features[0].items.len(), 8);
    assert_eq!(
        features[0].items[0].title,
        "programming rust 2nd edition pdf"
    );
}

#[tokio::test]
async fn brave_parses_snippet_serp() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "rust"))
        .and(query_param("source", "web"))
        .respond_with(ok(r#"<div class="snippet">
  <a href="https://www.rust-lang.org/"><span class="title">Rust</span></a>
  <span class="content">A language.</span>
</div>"#))
        .expect(1)
        .mount(&server)
        .await;
    let (hits, features) = Brave::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect("brave");
    assert!(features.is_empty());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Rust");
    assert_eq!(hits[0].url, "https://www.rust-lang.org/");
    assert_eq!(hits[0].snippet, "A language.");
    assert_eq!(hits[0].published, None);
}

#[tokio::test]
async fn qwant_parses_web_json_with_searxng_param_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/web"))
        .and(query_param("q", "rust"))
        .respond_with(ok(
            r#"{"status":"success","data":{"result":{"items":{"mainline":[
              {"type":"web","items":[
                {"title":"Rust","url":"https://www.rust-lang.org/","desc":"A language."}
              ]}
            ]}}}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let (hits, features) = Qwant::at(&server.uri())
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect("qwant");
    assert!(features.is_empty());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Rust");
    assert_eq!(hits[0].url, "https://www.rust-lang.org/");
    assert_eq!(hits[0].snippet, "A language.");
    let requests = server.received_requests().await.expect("recording on");
    assert_eq!(
        requests[0].url.query(),
        Some(
            "q=rust&count=10&locale=en_US&offset=0&tgp=1&device=desktop&safesearch=0&displayed=true&llm=true"
        )
    );
    assert!(requests[0].headers.get("cookie").is_none());
}

#[tokio::test]
async fn qwant_datadome_captcha_is_blocked_and_cookie_is_replayed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/web"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header(
                    "set-cookie",
                    "datadome=9CAGUy9c~B7ACmzWB00F8; Max-Age=31536000; Domain=.qwant.com; Path=/; Secure; SameSite=Lax",
                )
                .set_body_string(QWANT_CAPTCHA),
        )
        .expect(2)
        .mount(&server)
        .await;
    let qwant = Qwant::at(&server.uri());
    let client = reqwest::Client::new();
    let err = qwant
        .search(&client, &query("rust"))
        .await
        .expect_err("captcha");
    assert!(matches!(err, ProviderError::Blocked(_)), "{err}");
    let _ = qwant.search(&client, &query("rust")).await;
    assert_eq!(
        header_of(&server, 1, "cookie").await,
        "datadome=9CAGUy9c~B7ACmzWB00F8"
    );
}

#[tokio::test]
async fn qwant_api_error_is_an_error_and_recency_is_unsupported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/web"))
        .respond_with(ok(
            r#"{"status":"error","data":{"error_code":7,"message":["invalid query"]}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let qwant = Qwant::at(&server.uri());
    let err = qwant
        .search(&reqwest::Client::new(), &query("rust"))
        .await
        .expect_err("api error");
    assert!(
        matches!(&err, ProviderError::Parse(m) if m.contains("invalid query") && m.contains('7')),
        "{err}"
    );

    let mut q = query("rust");
    q.recent = Some(Recency::Month);
    let err = qwant
        .search(&reqwest::Client::new(), &q)
        .await
        .expect_err("no time filter");
    assert!(matches!(err, ProviderError::Unsupported), "{err}");
}
