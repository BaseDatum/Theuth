//! Brave Search API (16 §3.2): `GET /res/v1/web/search`, header
//! `X-Subscription-Token`. Results from `web.results[]`; features from the
//! `infobox`, `faq`, `news`, `videos`, `discussions` blocks and
//! `query.altered`. JSON fields only, no scraping. Provider name `brave_api`:
//! `brave` is the keyless `search.brave.com` scraper built into `web`.

use std::time::Duration;

use serde::Deserialize;
use web::{
    Category, Feature, FeatureItem, FeatureKind, Link, Provider, ProviderError, ProviderMeta,
    Query, RawResult, Recency,
};

use crate::{http_error, non_empty, parse, site_query, unescape};

/// Hosted endpoint.
pub const SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

const NAME: &str = "brave_api";
/// Brave's `count` hard maximum.
const MAX_COUNT: usize = 20;

/// Brave Search provider; needs a subscription token.
#[derive(Debug, Clone)]
pub struct Brave {
    key: String,
    url: String,
    timeout: Duration,
}

impl Brave {
    /// Provider against the hosted endpoint.
    #[must_use]
    pub fn new(key: &str, timeout: Duration) -> Self {
        Self {
            key: key.to_string(),
            url: SEARCH_URL.to_string(),
            timeout,
        }
    }

    /// Point at a different search URL (tests).
    #[must_use]
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
        self
    }
}

fn freshness(r: Recency) -> &'static str {
    match r {
        Recency::Day => "pd",
        Recency::Week => "pw",
        Recency::Month => "pm",
        Recency::Year => "py",
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    query: Option<QueryInfo>,
    web: Option<Block<Hit>>,
    news: Option<Block<Hit>>,
    videos: Option<Block<Hit>>,
    discussions: Option<Block<Hit>>,
    faq: Option<Block<FaqHit>>,
    infobox: Option<Block<Infobox>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QueryInfo {
    altered: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Block<T> {
    results: Vec<T>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
    age: Option<String>,
    page_age: Option<String>,
}

impl Hit {
    /// `page_age` is ISO-8601; `age` is "2 days ago" or a printed date. Both
    /// normalise downstream; the machine one wins.
    fn date(&self) -> Option<String> {
        non_empty(self.page_age.clone()).or_else(|| non_empty(self.age.clone()))
    }

    fn snippet(&self) -> String {
        self.description
            .as_deref()
            .map(unescape)
            .unwrap_or_default()
    }

    fn item(&self) -> Option<FeatureItem> {
        Some(FeatureItem {
            title: non_empty(self.title.clone())?,
            text: self.snippet(),
            url: non_empty(self.url.clone()),
            date: self.date(),
        })
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FaqHit {
    question: Option<String>,
    answer: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Infobox {
    title: Option<String>,
    description: Option<String>,
    long_desc: Option<String>,
    url: Option<String>,
    website_url: Option<String>,
    thumbnail: Option<Thumbnail>,
    attributes: Vec<Attribute>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Thumbnail {
    src: Option<String>,
}

/// Brave sends infobox rows as `[label, value]` pairs.
#[derive(Deserialize)]
#[serde(untagged)]
enum Attribute {
    Pair(Vec<String>),
    Labeled { label: String, value: String },
}

impl Attribute {
    fn into_row(self) -> Option<(String, String)> {
        match self {
            Attribute::Pair(mut p) if p.len() >= 2 => {
                let value = p.swap_remove(1);
                Some((p.swap_remove(0), value))
            }
            Attribute::Pair(_) => None,
            Attribute::Labeled { label, value } => Some((label, value)),
        }
    }
}

fn feature(kind: FeatureKind) -> Feature {
    Feature {
        kind,
        provider: NAME.to_string(),
        ..Feature::default()
    }
}

fn list(kind: FeatureKind, block: Option<Block<Hit>>) -> Option<Feature> {
    let items: Vec<_> = block?.results.iter().filter_map(Hit::item).collect();
    (!items.is_empty()).then(|| Feature {
        items,
        ..feature(kind)
    })
}

/// Map one Brave response body to results + features.
fn map(body: Response) -> (Vec<RawResult>, Vec<Feature>) {
    let results = body
        .web
        .map(|w| w.results)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|h| {
            Some(RawResult {
                title: non_empty(h.title.clone())?,
                url: non_empty(h.url.clone())?,
                snippet: h.snippet(),
                published: h.date(),
                score_hint: None,
            })
        })
        .collect();

    let mut features = Vec::new();
    if let Some(altered) = body.query.and_then(|q| non_empty(q.altered)) {
        features.push(Feature {
            text: Some(altered),
            ..feature(FeatureKind::Spelling)
        });
    }
    for ib in body.infobox.map(|b| b.results).unwrap_or_default() {
        let url = non_empty(ib.url).or_else(|| non_empty(ib.website_url));
        features.push(Feature {
            title: non_empty(ib.title),
            text: non_empty(ib.long_desc).or_else(|| non_empty(ib.description)),
            links: url
                .map(|u| Link {
                    title: "source".into(),
                    url: u,
                })
                .into_iter()
                .collect(),
            image: ib.thumbnail.and_then(|t| non_empty(t.src)),
            attributes: ib
                .attributes
                .into_iter()
                .filter_map(Attribute::into_row)
                .collect(),
            ..feature(FeatureKind::KnowledgePanel)
        });
    }
    if let Some(faq) = body.faq {
        let items: Vec<_> = faq
            .results
            .into_iter()
            .filter_map(|f| {
                Some(FeatureItem {
                    title: non_empty(f.question)?,
                    text: f.answer.as_deref().map(unescape).unwrap_or_default(),
                    url: non_empty(f.url),
                    date: None,
                })
            })
            .collect();
        if !items.is_empty() {
            features.push(Feature {
                items,
                ..feature(FeatureKind::Faq)
            });
        }
    }
    features.extend(list(FeatureKind::News, body.news));
    features.extend(list(FeatureKind::Videos, body.videos));
    features.extend(list(FeatureKind::Discussions, body.discussions));
    (results, features)
}

#[async_trait::async_trait]
impl Provider for Brave {
    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }

    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: NAME,
            categories: &[Category::General, Category::News, Category::It],
            features: &[
                FeatureKind::KnowledgePanel,
                FeatureKind::Faq,
                FeatureKind::News,
                FeatureKind::Videos,
                FeatureKind::Discussions,
                FeatureKind::Spelling,
            ],
            renders: false,
            billable: true,
        }
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let count = q.limit.clamp(1, MAX_COUNT).to_string();
        let mut params = vec![
            ("q", site_query(q)),
            ("count", count),
            ("text_decorations", "false".to_string()),
        ];
        if let Some(r) = q.recent {
            params.push(("freshness", freshness(r).to_string()));
        }
        let resp = client
            .get(&self.url)
            .timeout(self.timeout)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.key)
            .query(&params)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(http_error(status));
        }
        let body = resp.text().await?;
        Ok(map(parse(&body)?))
    }
}
