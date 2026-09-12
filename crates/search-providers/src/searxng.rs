//! SearXNG (16 §3.2): self-hosted metasearch, `GET {base}/search?format=json`.
//! Results from `results[]`; features from `answers[]` (Answer),
//! `infoboxes[]` (KnowledgePanel), `suggestions[]` (Related) and
//! `corrections[]` (Spelling). Keyless: the instance is the trust boundary.

use serde::Deserialize;
use web::{
    Category, Feature, FeatureItem, FeatureKind, Link, Provider, ProviderError, ProviderMeta,
    Query, RawResult, Recency,
};

use crate::{http_error, non_empty, parse, site_query};

const NAME: &str = "searxng";

/// SearXNG provider against one instance.
#[derive(Debug, Clone)]
pub struct Searxng {
    base: String,
}

impl Searxng {
    /// `base` is the instance root without a trailing slash.
    #[must_use]
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
        }
    }
}

fn category(c: Category) -> &'static str {
    match c {
        Category::General => "general",
        Category::News => "news",
        Category::It => "it",
        Category::Science => "science",
    }
}

fn time_range(r: Recency) -> &'static str {
    match r {
        Recency::Day => "day",
        Recency::Week => "week",
        Recency::Month => "month",
        Recency::Year => "year",
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    results: Vec<Hit>,
    answers: Vec<Answer>,
    infoboxes: Vec<Infobox>,
    suggestions: Vec<String>,
    corrections: Vec<String>,
    /// `[[name, reason], …]`; some versions append extra columns, so kept opaque.
    unresponsive_engines: Vec<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
    #[serde(rename = "publishedDate")]
    published_date: Option<String>,
    score: Option<f64>,
}

/// Older instances send bare strings, newer ones `{answer, url}`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Answer {
    Text(String),
    Sourced {
        answer: String,
        #[serde(default)]
        url: Option<String>,
    },
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Infobox {
    infobox: Option<String>,
    content: Option<String>,
    img_src: Option<String>,
    urls: Vec<InfoboxUrl>,
    attributes: Vec<Attribute>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct InfoboxUrl {
    title: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Attribute {
    label: Option<String>,
    value: Option<String>,
}

fn feature(kind: FeatureKind) -> Feature {
    Feature {
        kind,
        provider: NAME.to_string(),
        ..Feature::default()
    }
}

fn terms(kind: FeatureKind, words: Vec<String>) -> Option<Feature> {
    let items: Vec<_> = words
        .into_iter()
        .filter_map(|t| non_empty(Some(t)))
        .map(|title| FeatureItem {
            title,
            ..FeatureItem::default()
        })
        .collect();
    (!items.is_empty()).then(|| Feature {
        items,
        ..feature(kind)
    })
}

fn map(body: Response, limit: usize) -> (Vec<RawResult>, Vec<Feature>) {
    if !body.unresponsive_engines.is_empty() {
        tracing::debug!(engines = ?body.unresponsive_engines, "searxng: unresponsive engines");
    }
    let results = body
        .results
        .into_iter()
        .filter_map(|h| {
            Some(RawResult {
                title: non_empty(h.title)?,
                url: non_empty(h.url)?,
                snippet: h.content.unwrap_or_default(),
                published: non_empty(h.published_date),
                score_hint: h.score,
            })
        })
        .take(limit)
        .collect();

    let mut features = Vec::new();
    for a in body.answers {
        let (text, url) = match a {
            Answer::Text(t) => (t, None),
            Answer::Sourced { answer, url } => (answer, non_empty(url)),
        };
        let Some(text) = non_empty(Some(text)) else {
            continue;
        };
        features.push(Feature {
            text: Some(text),
            links: url
                .map(|u| Link {
                    title: "source".into(),
                    url: u,
                })
                .into_iter()
                .collect(),
            ..feature(FeatureKind::Answer)
        });
    }
    for ib in body.infoboxes {
        features.push(Feature {
            title: non_empty(ib.infobox),
            text: non_empty(ib.content),
            links: ib
                .urls
                .into_iter()
                .filter_map(|u| {
                    Some(Link {
                        url: non_empty(u.url)?,
                        title: non_empty(u.title).unwrap_or_default(),
                    })
                })
                .collect(),
            image: non_empty(ib.img_src),
            attributes: ib
                .attributes
                .into_iter()
                .filter_map(|a| Some((non_empty(a.label)?, non_empty(a.value)?)))
                .collect(),
            ..feature(FeatureKind::KnowledgePanel)
        });
    }
    features.extend(terms(FeatureKind::Related, body.suggestions));
    features.extend(terms(FeatureKind::Spelling, body.corrections));
    (results, features)
}

#[async_trait::async_trait]
impl Provider for Searxng {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: NAME,
            categories: &[
                Category::General,
                Category::News,
                Category::It,
                Category::Science,
            ],
            features: &[
                FeatureKind::Answer,
                FeatureKind::KnowledgePanel,
                FeatureKind::Related,
                FeatureKind::Spelling,
            ],
            renders: false,
            billable: false,
        }
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let mut params = vec![
            ("q", site_query(q)),
            ("format", "json".to_string()),
            ("categories", category(q.category).to_string()),
            ("pageno", "1".to_string()),
        ];
        if let Some(r) = q.recent {
            params.push(("time_range", time_range(r).to_string()));
        }
        let resp = client
            .get(format!("{}/search", self.base))
            .query(&params)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(http_error(status));
        }
        let body = resp.text().await?;
        Ok(map(parse(&body)?, q.limit))
    }
}
