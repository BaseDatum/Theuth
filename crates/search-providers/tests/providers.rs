//! One wiremock contract test per provider (16 §8): the request the provider
//! sends, the results/features it maps from the documented response shape,
//! and the HTTP status → `ProviderError` mapping.

use std::collections::BTreeSet;

use search_providers::brave::Brave;
use search_providers::firecrawl::Firecrawl;
use search_providers::openserp::Openserp;
use search_providers::searxng::Searxng;
use serde_json::json;
use web::{Category, FeatureKind, Provider, ProviderError, Query, Recency};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn query(category: Category) -> Query {
    Query {
        text: "rust async trait".into(),
        category,
        limit: 5,
        site: Some("example.com".into()),
        recent: Some(Recency::Week),
    }
}

fn json_body(fixture: &'static str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture, "application/json")
}

fn kinds(features: &[web::Feature]) -> BTreeSet<FeatureKind> {
    features.iter().map(|f| f.kind).collect()
}

#[tokio::test]
async fn brave_maps_results_features_and_statuses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .and(header("X-Subscription-Token", "test-key"))
        .and(query_param("q", "site:example.com rust async trait"))
        .and(query_param("count", "5"))
        .and(query_param("freshness", "pw"))
        .respond_with(json_body(include_str!("fixtures/brave.json")))
        .expect(1)
        .mount(&server)
        .await;
    let brave = Brave::new("test-key").with_url(&format!("{}/res/v1/web/search", server.uri()));
    let client = reqwest::Client::new();

    let (results, features) = brave
        .search(&client, &query(Category::General))
        .await
        .expect("search");
    assert_eq!(results.len(), 2, "title-less hit dropped");
    assert_eq!(results[0].url, "https://docs.rs/async-trait");
    assert_eq!(
        results[0].snippet,
        "Type erasure for async trait methods & more."
    );
    assert_eq!(results[1].published.as_deref(), Some("2023-12-21T00:00:00"));
    assert_eq!(
        kinds(&features),
        BTreeSet::from([
            FeatureKind::Spelling,
            FeatureKind::KnowledgePanel,
            FeatureKind::Faq,
            FeatureKind::News,
            FeatureKind::Videos,
            FeatureKind::Discussions,
        ])
    );
    let panel = features
        .iter()
        .find(|f| f.kind == FeatureKind::KnowledgePanel)
        .expect("panel");
    assert_eq!(
        panel.attributes,
        [
            ("Designed by".to_string(), "Graydon Hoare".to_string()),
            ("First appeared".into(), "2015".into())
        ]
    );
    assert_eq!(
        panel.image.as_deref(),
        Some("https://imgs.search.brave.com/rust-logo.png")
    );
    let faq = features
        .iter()
        .find(|f| f.kind == FeatureKind::Faq)
        .expect("faq");
    assert_eq!(
        faq.items[0].title,
        "Can traits have async functions in Rust?"
    );
    let news = features
        .iter()
        .find(|f| f.kind == FeatureKind::News)
        .expect("news");
    assert_eq!(news.items[0].date.as_deref(), Some("2 years ago"));
    assert!(features.iter().all(|f| f.provider == "brave"));

    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    assert!(matches!(
        brave.search(&client, &query(Category::General)).await,
        Err(ProviderError::RateLimited)
    ));
}

#[tokio::test]
async fn firecrawl_search_render_and_statuses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/search"))
        .and(header("Authorization", "Bearer fc-key"))
        .and(body_partial_json(json!({
            "query": "rust async trait",
            "limit": 5,
            "sources": [{"type": "web"}, {"type": "news"}, {"type": "images"}],
            "tbs": "qdr:w",
            "includeDomains": ["example.com"]
        })))
        .respond_with(json_body(include_str!("fixtures/firecrawl_search.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/scrape"))
        .and(header("Authorization", "Bearer fc-key"))
        .and(body_partial_json(
            json!({"url": "https://example.com/", "formats": ["markdown"]}),
        ))
        .respond_with(json_body(include_str!("fixtures/firecrawl_scrape.json")))
        .expect(1)
        .mount(&server)
        .await;
    let fc = Firecrawl::new(&format!("{}/", server.uri()), "fc-key");
    assert!(fc.meta().renders);
    let client = reqwest::Client::new();

    let (results, features) = fc
        .search(&client, &query(Category::General))
        .await
        .expect("search");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "async_trait - Rust");
    assert_eq!(
        kinds(&features),
        BTreeSet::from([FeatureKind::News, FeatureKind::Images])
    );
    let news = features
        .iter()
        .find(|f| f.kind == FeatureKind::News)
        .expect("news");
    assert_eq!(news.items[0].date.as_deref(), Some("2023-12-28"));
    let images = features
        .iter()
        .find(|f| f.kind == FeatureKind::Images)
        .expect("images");
    assert_eq!(
        images.items[0].url.as_deref(),
        Some("https://www.rust-lang.org/static/images/rust-logo-blk.svg")
    );

    let page = fc
        .render(
            &client,
            &url::Url::parse("https://example.com/").expect("url"),
        )
        .await
        .expect("render");
    assert_eq!(page.title.as_deref(), Some("Example Domain"));
    assert!(page.markdown.starts_with("# Example Domain"));

    // News category: news hits are the results, not a feature.
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/v2/search"))
        .and(body_partial_json(json!({"sources": [{"type": "news"}]})))
        .respond_with(json_body(include_str!("fixtures/firecrawl_search.json")))
        .mount(&server)
        .await;
    let (results, features) = fc
        .search(&client, &query(Category::News))
        .await
        .expect("news search");
    assert_eq!(results[0].title, "Rust 1.75 released");
    assert_eq!(kinds(&features), BTreeSet::from([FeatureKind::Images]));

    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402))
        .mount(&server)
        .await;
    match fc.search(&client, &query(Category::General)).await {
        Err(ProviderError::Http(msg)) => assert!(msg.contains("payment required"), "{msg}"),
        other => panic!("expected Http, got {other:?}"),
    }

    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    assert!(matches!(
        fc.search(&client, &query(Category::General)).await,
        Err(ProviderError::Unauthorized)
    ));
}

#[tokio::test]
async fn searxng_maps_results_and_features() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "site:example.com rust async trait"))
        .and(query_param("format", "json"))
        .and(query_param("categories", "science"))
        .and(query_param("time_range", "week"))
        .respond_with(json_body(include_str!("fixtures/searxng.json")))
        .expect(1)
        .mount(&server)
        .await;
    let sx = Searxng::new(&server.uri());
    assert!(!sx.meta().billable);

    let (results, features) = sx
        .search(&reqwest::Client::new(), &query(Category::Science))
        .await
        .expect("search");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].score_hint, Some(2.0));
    assert_eq!(results[0].published.as_deref(), Some("2025-11-02T00:00:00"));
    assert_eq!(results[1].published, None);
    assert_eq!(
        kinds(&features),
        BTreeSet::from([
            FeatureKind::Answer,
            FeatureKind::KnowledgePanel,
            FeatureKind::Related,
            FeatureKind::Spelling
        ])
    );
    let answers: Vec<_> = features
        .iter()
        .filter(|f| f.kind == FeatureKind::Answer)
        .collect();
    assert_eq!(answers.len(), 2, "string and object answers both map");
    assert_eq!(answers[1].links[0].url, "https://blog.rust-lang.org/");
    let panel = features
        .iter()
        .find(|f| f.kind == FeatureKind::KnowledgePanel)
        .expect("panel");
    assert_eq!(panel.title.as_deref(), Some("Rust"));
    assert_eq!(panel.attributes.len(), 2);
    assert_eq!(panel.links.len(), 2);
    let related = features
        .iter()
        .find(|f| f.kind == FeatureKind::Related)
        .expect("related");
    assert_eq!(related.items.len(), 2);
    let spelling = features
        .iter()
        .find(|f| f.kind == FeatureKind::Spelling)
        .expect("spelling");
    assert_eq!(spelling.items[0].title, "rust async traits");
}

#[tokio::test]
async fn openserp_maps_serp_features_and_renders() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mega/search"))
        .and(header("Authorization", "Bearer os-key"))
        .and(query_param("engines", "google,bing"))
        .and(query_param("text", "rust async trait"))
        .and(query_param("site", "example.com"))
        .and(query_param("limit", "5"))
        .respond_with(json_body(include_str!("fixtures/openserp.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/extract"))
        .and(query_param("url", "https://example.com/"))
        .respond_with(json_body(include_str!("fixtures/openserp_extract.json")))
        .expect(1)
        .mount(&server)
        .await;
    let os = Openserp::new(
        &server.uri(),
        &["google".into(), "bing".into()],
        Some("os-key"),
    );
    assert!(os.meta().renders);
    let client = reqwest::Client::new();

    let (results, features) = os
        .search(&client, &query(Category::General))
        .await
        .expect("search");
    assert_eq!(results.len(), 2);
    assert_eq!(results[1].title, "Async fn in trait");
    assert_eq!(
        kinds(&features),
        BTreeSet::from([
            FeatureKind::AiSummary,
            FeatureKind::Answer,
            FeatureKind::Faq,
            FeatureKind::Related,
            FeatureKind::News,
            FeatureKind::KnowledgePanel,
            FeatureKind::Videos,
            FeatureKind::Images,
        ]),
        "shopping is not carried"
    );
    let ai = features
        .iter()
        .find(|f| f.kind == FeatureKind::AiSummary)
        .expect("ai");
    assert_eq!(ai.links[0].url, "https://blog.rust-lang.org/");
    let faq = features
        .iter()
        .find(|f| f.kind == FeatureKind::Faq)
        .expect("faq");
    assert_eq!(faq.items.len(), 2);
    assert_eq!(faq.items[0].url.as_deref(), Some("https://example.com/a"));

    let page = os
        .render(
            &client,
            &url::Url::parse("https://example.com/").expect("url"),
        )
        .await
        .expect("render");
    assert_eq!(page.title.as_deref(), Some("Example Domain"));
    assert!(page.markdown.contains("illustrative"));

    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    assert!(matches!(
        os.search(&client, &query(Category::General)).await,
        Err(ProviderError::Blocked(_))
    ));
}
