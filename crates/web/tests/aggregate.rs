//! Aggregator behaviour with mock providers (design doc 16 §3 fan-out, §4.3
//! feature dedupe, §4.6 drift): no network.

use std::sync::Arc;
use std::time::Duration;

use web::aggregate::{self, Drift, merge_features, merge_rank_dedupe};
use web::health::Health;
use web::{
    Category, Feature, FeatureItem, FeatureKind, Provider, ProviderError, ProviderMeta, Query,
    RawResult, WebConfig,
};

struct MockProvider {
    name: &'static str,
    categories: &'static [Category],
    delay: Duration,
    results: Vec<RawResult>,
    features: Vec<Feature>,
    error: Option<ProviderError>,
}

impl MockProvider {
    fn ok(name: &'static str, results: Vec<RawResult>) -> Self {
        Self {
            name,
            categories: &[Category::General],
            delay: Duration::ZERO,
            results,
            features: vec![],
            error: None,
        }
    }

    fn slow(name: &'static str, delay: Duration, results: Vec<RawResult>) -> Self {
        Self {
            delay,
            ..Self::ok(name, results)
        }
    }

    fn fail(name: &'static str, message: &str) -> Self {
        Self {
            error: Some(ProviderError::Http(message.to_owned())),
            ..Self::ok(name, vec![])
        }
    }

    fn unsupported(name: &'static str) -> Self {
        Self {
            error: Some(ProviderError::Unsupported),
            ..Self::ok(name, vec![])
        }
    }

    fn with_features(mut self, features: Vec<Feature>) -> Self {
        self.features = features;
        self
    }

    fn with_categories(mut self, categories: &'static [Category]) -> Self {
        self.categories = categories;
        self
    }

    fn arc(self) -> Arc<dyn Provider> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: self.name,
            categories: self.categories,
            features: &[],
            renders: false,
            billable: false,
        }
    }

    async fn search(
        &self,
        _client: &reqwest::Client,
        _q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if let Some(e) = &self.error {
            return Err(match e {
                ProviderError::Unsupported => ProviderError::Unsupported,
                other => ProviderError::Http(other.to_string()),
            });
        }
        Ok((self.results.clone(), self.features.clone()))
    }
}

fn hit(title: &str, url: &str, snippet: &str) -> RawResult {
    RawResult {
        title: title.into(),
        url: url.into(),
        snippet: snippet.into(),
        ..Default::default()
    }
}

fn query(text: &str) -> Query {
    Query {
        text: text.into(),
        category: Category::General,
        limit: 8,
        site: None,
        recent: None,
    }
}

fn cfg(timeout_ms: u64, overall_timeout_ms: u64) -> WebConfig {
    WebConfig {
        timeout_ms,
        overall_timeout_ms,
        ..WebConfig::default()
    }
}

async fn run(
    providers: &[Arc<dyn Provider>],
    q: &Query,
    cfg: &WebConfig,
) -> aggregate::SearchResponse {
    aggregate::search(
        providers,
        &reqwest::Client::new(),
        q,
        None,
        cfg,
        &Drift::default(),
        &Health::default(),
    )
    .await
    .expect("selection ok")
}

// ── merge / rank ──────────────────────────────────────────────────────────

#[test]
fn shared_url_at_rank_5_beats_single_provider_rank_1() {
    let mut a = Vec::new();
    let mut b = Vec::new();
    for i in 1..=4 {
        a.push(hit(
            &format!("A{i}"),
            &format!("https://example.com/a{i}"),
            "x",
        ));
        b.push(hit(
            &format!("B{i}"),
            &format!("https://example.com/b{i}"),
            "x",
        ));
    }
    a.push(hit(
        "Shared",
        "https://example.com/shared?utm_source=t",
        "both",
    ));
    b.push(hit("Shared", "https://example.com/shared/", "both"));

    let ranked = merge_rank_dedupe(&[("a".into(), a), ("b".into(), b)], 10);
    assert!(
        ranked[0].url.starts_with("https://example.com/shared"),
        "{}",
        ranked[0].url
    );
    assert!(!ranked[0].url.contains("utm_"));
    assert_eq!(ranked[0].providers, vec!["a", "b"]);
    assert_eq!(ranked[0].rank, 1);
    assert_eq!(ranked.len(), 9);
}

#[test]
fn tracking_variants_merge_and_blank_fields_fill() {
    let hits = vec![
        (
            "ddg".to_owned(),
            vec![hit("Rust", "https://www.rust-lang.org/?utm_campaign=x", "")],
        ),
        (
            "wiki".to_owned(),
            vec![hit(
                "Rust lang",
                "https://www.rust-lang.org",
                "longer snippet here",
            )],
        ),
    ];
    let ranked = merge_rank_dedupe(&hits, 5);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].providers, vec!["ddg", "wiki"]);
    assert_eq!(
        ranked[0].title, "Rust",
        "best-ranked title wins on equal rank (first seen)"
    );
    assert_eq!(
        ranked[0].snippet, "longer snippet here",
        "blank snippet filled from the other hit"
    );
    assert!(!ranked[0].url.contains("utm_"));
}

#[test]
fn within_provider_dupes_do_not_double_count() {
    // `noisy` lists the same document twice; if both ranks counted, dup would
    // outscore clean's rank-1 hit. With one term each they tie and the shorter
    // URL wins.
    let hits = vec![
        (
            "noisy".to_owned(),
            vec![
                hit("A", "https://example.com/dup", "first"),
                hit("A2", "https://example.com/dup?utm_source=x", "second"),
            ],
        ),
        (
            "clean".to_owned(),
            vec![hit("X", "https://example.com/x", "s")],
        ),
    ];
    let out = merge_rank_dedupe(&hits, 5);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].url, "https://example.com/x");
}

#[test]
fn limit_is_respected() {
    let hits: Vec<RawResult> = (1..=10)
        .map(|i| hit(&format!("T{i}"), &format!("https://example.com/{i}"), "s"))
        .collect();
    let out = merge_rank_dedupe(&[("a".into(), hits)], 3);
    assert_eq!(out.len(), 3);
    assert_eq!(out[2].rank, 3);
}

// ── fan-out ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn partial_failure_returns_ok_results() {
    let providers = [
        MockProvider::ok("fast", vec![hit("One", "https://example.com/1", "s")]).arc(),
        MockProvider::fail("broken", "bot wall").arc(),
    ];
    let resp = run(&providers, &query("rust"), &cfg(1000, 2000)).await;
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.ok, vec!["fast"]);
    assert_eq!(resp.failed.len(), 1);
    assert_eq!(resp.failed[0].0, "broken");
    assert!(resp.failed[0].1.contains("bot wall"));
}

#[tokio::test]
async fn unsupported_provider_is_skipped_silently() {
    let providers = [
        MockProvider::ok("fast", vec![hit("One", "https://example.com/1", "s")]).arc(),
        MockProvider::unsupported("no_recency").arc(),
    ];
    let resp = run(&providers, &query("rust"), &cfg(1000, 2000)).await;
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.ok, vec!["fast"]);
    assert!(resp.failed.is_empty(), "{:?}", resp.failed);
}

#[tokio::test]
async fn per_provider_timeout_does_not_block_others() {
    let providers = [
        MockProvider::ok("fast", vec![hit("Fast", "https://example.com/fast", "s")]).arc(),
        MockProvider::slow(
            "slow",
            Duration::from_millis(800),
            vec![hit("Slow", "https://example.com/slow", "s")],
        )
        .arc(),
    ];
    let resp = run(&providers, &query("q"), &cfg(50, 400)).await;
    assert_eq!(resp.ok, vec!["fast"]);
    assert_eq!(resp.results[0].url, "https://example.com/fast");
    assert!(
        resp.failed
            .iter()
            .any(|(n, e)| n == "slow" && e.contains("timed out")),
        "{:?}",
        resp.failed
    );
}

#[tokio::test]
async fn overall_deadline_marks_stragglers_failed() {
    let providers = [
        MockProvider::slow(
            "slow_a",
            Duration::from_millis(500),
            vec![hit("A", "https://example.com/a", "s")],
        )
        .arc(),
        MockProvider::slow(
            "slow_b",
            Duration::from_millis(500),
            vec![hit("B", "https://example.com/b", "s")],
        )
        .arc(),
        MockProvider::ok("instant", vec![hit("I", "https://example.com/i", "s")]).arc(),
    ];
    let resp = run(&providers, &query("q"), &cfg(5000, 80)).await;
    assert_eq!(resp.ok, vec!["instant"]);
    assert_eq!(resp.results[0].url, "https://example.com/i");
    assert_eq!(resp.failed.len(), 2, "{:?}", resp.failed);
    assert!(resp.failed.iter().all(|(_, e)| e.contains("deadline")));
}

#[tokio::test]
async fn all_failed_is_reported_not_fatal() {
    let providers = [
        MockProvider::fail("a", "down").arc(),
        MockProvider::fail("b", "down").arc(),
    ];
    let resp = run(&providers, &query("q"), &cfg(1000, 2000)).await;
    assert!(resp.ok.is_empty());
    assert!(resp.results.is_empty());
    assert_eq!(resp.failed.len(), 2);
}

#[tokio::test]
async fn selection_by_category_and_name() {
    let providers = [
        MockProvider::ok("gen", vec![hit("G", "https://example.com/g", "s")]).arc(),
        MockProvider::ok("sci", vec![hit("S", "https://example.com/s", "s")])
            .with_categories(&[Category::Science])
            .arc(),
    ];
    let c = cfg(1000, 2000);
    let resp = run(&providers, &query("q"), &c).await;
    assert_eq!(
        resp.ok,
        vec!["gen"],
        "science-only provider skipped for a general query"
    );

    let client = reqwest::Client::new();
    let nope = ["nope".to_owned()];
    let err = aggregate::search(
        &providers,
        &client,
        &query("q"),
        Some(&nope[..]),
        &c,
        &Drift::default(),
        &Health::default(),
    )
    .await
    .expect_err("unknown name");
    assert!(
        err.contains("nope") && err.contains("gen") && err.contains("sci"),
        "{err}"
    );

    let sci = ["sci".to_owned()];
    let err = aggregate::search(
        &providers,
        &client,
        &query("q"),
        Some(&sci[..]),
        &c,
        &Drift::default(),
        &Health::default(),
    )
    .await
    .expect_err("category mismatch");
    assert!(err.contains("sci") && err.contains("general"), "{err}");
}

// ── features ──────────────────────────────────────────────────────────────

fn related(provider: &str, title: &str, first: &str) -> Feature {
    Feature {
        kind: FeatureKind::Related,
        provider: provider.into(),
        title: Some(title.into()),
        items: vec![FeatureItem {
            title: first.into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn features_dedupe_by_key_and_follow_render_order() {
    let answer = Feature {
        kind: FeatureKind::Answer,
        provider: "b".into(),
        text: Some("42".into()),
        ..Default::default()
    };
    let merged = merge_features(
        vec![
            related("b", "Related Searches", "rust book"),
            answer.clone(),
            related("a", "related searches", "Rust Book"),
            related("a", "related searches", "other"),
        ],
        &["a".into(), "b".into()],
    );
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0], answer, "answer renders before related");
    assert_eq!(
        merged[1].provider, "a",
        "first-registered provider wins the duplicate"
    );
    assert_eq!(merged[2].items[0].title, "other");
}

#[tokio::test]
async fn features_from_providers_survive_search() {
    let providers = [MockProvider::ok("a", vec![])
        .with_features(vec![related("a", "r", "x")])
        .arc()];
    let resp = run(&providers, &query("q"), &cfg(1000, 2000)).await;
    assert_eq!(resp.features.len(), 1);
    assert_eq!(resp.ok, vec!["a"]);
}

// ── drift (16 §4.6) ───────────────────────────────────────────────────────

#[tokio::test]
async fn drift_detector_fires_on_third_empty_call_and_resets() {
    let empty = [MockProvider::ok("empty", vec![]).arc()];
    let c = cfg(1000, 2000);
    let client = reqwest::Client::new();
    let drift = Drift::default();
    for call in 1..=2 {
        let r = aggregate::search(
            &empty,
            &client,
            &query("q"),
            None,
            &c,
            &drift,
            &Health::default(),
        )
        .await
        .expect("ok");
        assert_eq!(r.ok, vec!["empty"], "call {call} still ok");
        assert!(r.failed.is_empty());
    }
    let r = aggregate::search(
        &empty,
        &client,
        &query("q"),
        None,
        &c,
        &drift,
        &Health::default(),
    )
    .await
    .expect("ok");
    assert!(r.ok.is_empty());
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].0, "empty");
    assert!(
        r.failed[0]
            .1
            .contains("no results; response shape may have changed")
    );

    // A yield resets the counter for the same name.
    let yielding = [MockProvider::ok("empty", vec![hit("Y", "https://example.com/y", "s")]).arc()];
    let r = aggregate::search(
        &yielding,
        &client,
        &query("q"),
        None,
        &c,
        &drift,
        &Health::default(),
    )
    .await
    .expect("ok");
    assert_eq!(r.ok, vec!["empty"]);
    let r = aggregate::search(
        &empty,
        &client,
        &query("q"),
        None,
        &c,
        &drift,
        &Health::default(),
    )
    .await
    .expect("ok");
    assert_eq!(
        r.ok,
        vec!["empty"],
        "one empty call after a yield is not drift"
    );
}

// ── degraded (16 §4.6, measured 2026-09-27) ───────────────────────────────

#[tokio::test]
async fn degraded_when_only_wikipedia_or_one_general_engine_answers() {
    let c = cfg(1000, 2000);
    let wiki = hit("Tokio", "https://en.wikipedia.org/wiki/Tokio", "");
    let providers = [
        MockProvider::ok("wikipedia", vec![wiki.clone()]).arc(),
        MockProvider::ok(
            "google",
            vec![hit("tokio JoinSet", "https://docs.rs/tokio", "")],
        )
        .arc(),
        MockProvider::fail("brave", "rate limited").arc(),
        MockProvider::fail("duckduckgo", "blocked: HTTP 202 challenge").arc(),
    ];
    let r = run(&providers, &query("tokio JoinSet"), &c).await;
    assert!(r.degraded, "one general engine + wikipedia is degraded");
    assert_eq!(r.reasons, ["brave: rate_limited", "duckduckgo: challenge"]);

    let providers = [
        MockProvider::ok(
            "google",
            vec![hit("tokio JoinSet", "https://docs.rs/tokio", "")],
        )
        .arc(),
        MockProvider::ok(
            "qwant",
            vec![hit("tokio JoinSet", "https://docs.rs/tokio", "")],
        )
        .arc(),
    ];
    let r = run(&providers, &query("tokio JoinSet"), &c).await;
    assert!(!r.degraded, "two general engines answering is fine");

    let providers = [
        MockProvider::ok(
            "searxng",
            vec![hit("tokio JoinSet", "https://docs.rs/tokio", "")],
        )
        .arc(),
        MockProvider::fail("google", "HTTP 429").arc(),
    ];
    let r = run(&providers, &query("tokio JoinSet"), &c).await;
    assert!(
        !r.degraded,
        "an external provider answering is never degraded"
    );
    assert_eq!(r.reasons, ["google: rate_limited"]);
}

#[tokio::test]
async fn keyless_fallback_off_skips_builtins_when_an_external_serves() {
    let c = WebConfig {
        keyless_fallback: false,
        ..cfg(1000, 2000)
    };
    let providers = [
        MockProvider::ok("google", vec![hit("g", "https://g.example/", "")]).arc(),
        MockProvider::ok("searxng", vec![hit("s", "https://s.example/", "")]).arc(),
    ];
    let r = run(&providers, &query("q"), &c).await;
    assert_eq!(r.ok, ["searxng"]);

    let only_builtin = [MockProvider::ok("google", vec![hit("g", "https://g.example/", "")]).arc()];
    let r = run(&only_builtin, &query("q"), &c).await;
    assert_eq!(r.ok, ["google"], "nothing external: builtins still run");
}

#[test]
fn published_is_rfc3339_or_absent() {
    let dated = RawResult {
        published: Some("March 12, 2024".into()),
        ..hit("A", "https://a.example/", "")
    };
    let junk = RawResult {
        published: Some("Install Bun with curl and read the release notes".into()),
        ..hit("B", "https://b.example/", "")
    };
    let ranked = merge_rank_dedupe(&[("p".to_owned(), vec![dated, junk])], 8);
    assert_eq!(ranked[0].published.as_deref(), Some("2024-03-12T00:00:00Z"));
    assert_eq!(ranked[1].published, None);
}

#[tokio::test]
async fn site_applies_to_the_returned_rows() {
    let c = cfg(1000, 2000);
    let providers = [MockProvider::ok(
        "google",
        vec![
            hit("PDF", "https://en.wikipedia.org/wiki/PDF", ""),
            hit("PrintToPdf", "https://learn.microsoft.com/x", ""),
        ],
    )
    .arc()];
    let q = Query {
        site: Some("learn.microsoft.com".into()),
        ..query("PrintToPdf")
    };
    let r = run(&providers, &q, &c).await;
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].url, "https://learn.microsoft.com/x");
    assert_eq!(r.results[0].rank, 1);
    assert_eq!(
        r.notes,
        ["1 off-site rows dropped (site=learn.microsoft.com)"]
    );
}
