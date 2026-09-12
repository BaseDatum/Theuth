//! Firecrawl (16 §3.2, §5): `POST {base}/v2/search` for results, news and
//! images; `POST {base}/v2/scrape` as the `rendered` fetch path. Hosted or
//! self-hosted (`base_url`); `Authorization: Bearer <key>`.

use serde::Deserialize;
use serde_json::json;
use web::{
    Category, Feature, FeatureItem, FeatureKind, Provider, ProviderError, ProviderMeta, Query,
    RawResult, Recency, Rendered,
};

use crate::{http_error, non_empty, parse};

const NAME: &str = "firecrawl";

/// Firecrawl provider; search and render.
#[derive(Debug, Clone)]
pub struct Firecrawl {
    base: String,
    key: String,
}

impl Firecrawl {
    /// `base` is the API root without a trailing slash (`https://api.firecrawl.dev`).
    #[must_use]
    pub fn new(base: &str, key: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
        }
    }

    async fn post(
        &self,
        client: &reqwest::Client,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<String, ProviderError> {
        let resp = client
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if status.as_u16() == 402 {
            return Err(ProviderError::Http(
                "payment required: free credits exhausted".into(),
            ));
        }
        if !status.is_success() {
            return Err(http_error(status));
        }
        Ok(resp.text().await?)
    }
}

fn tbs(r: Recency) -> &'static str {
    match r {
        Recency::Day => "qdr:d",
        Recency::Week => "qdr:w",
        Recency::Month => "qdr:m",
        Recency::Year => "qdr:y",
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

impl<T> Envelope<T> {
    fn data(self) -> Result<Option<T>, ProviderError> {
        if !self.success {
            return Err(ProviderError::Http(
                self.error.unwrap_or_else(|| "success: false".into()),
            ));
        }
        Ok(self.data)
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchData {
    web: Vec<WebHit>,
    news: Vec<NewsHit>,
    images: Vec<ImageHit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WebHit {
    url: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewsHit {
    title: Option<String>,
    url: Option<String>,
    snippet: Option<String>,
    date: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ImageHit {
    title: Option<String>,
    image_url: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ScrapeData {
    markdown: Option<String>,
    metadata: Metadata,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Metadata {
    title: Option<String>,
    #[serde(rename = "sourceURL")]
    source_url: Option<String>,
}

fn feature(kind: FeatureKind, items: Vec<FeatureItem>) -> Option<Feature> {
    (!items.is_empty()).then(|| Feature {
        kind,
        provider: NAME.to_string(),
        items,
        ..Feature::default()
    })
}

fn map(data: SearchData, category: Category) -> (Vec<RawResult>, Vec<Feature>) {
    let news: Vec<RawResult> = data
        .news
        .into_iter()
        .filter_map(|n| {
            Some(RawResult {
                title: non_empty(n.title)?,
                url: non_empty(n.url)?,
                snippet: n.snippet.unwrap_or_default(),
                published: non_empty(n.date),
                score_hint: None,
            })
        })
        .collect();
    let web: Vec<RawResult> = data
        .web
        .into_iter()
        .filter_map(|w| {
            Some(RawResult {
                title: non_empty(w.title)?,
                url: non_empty(w.url)?,
                snippet: w.description.unwrap_or_default(),
                published: None,
                score_hint: None,
            })
        })
        .collect();

    let mut features = Vec::new();
    let results = if category == Category::News {
        news
    } else {
        features.extend(feature(
            FeatureKind::News,
            news.into_iter()
                .map(|r| FeatureItem {
                    title: r.title,
                    text: r.snippet,
                    url: Some(r.url),
                    date: r.published,
                })
                .collect(),
        ));
        web
    };
    features.extend(feature(
        FeatureKind::Images,
        data.images
            .into_iter()
            .filter_map(|i| {
                Some(FeatureItem {
                    title: non_empty(i.title).unwrap_or_default(),
                    text: String::new(),
                    url: Some(non_empty(i.image_url).or_else(|| non_empty(i.url))?),
                    date: None,
                })
            })
            .collect(),
    ));
    (results, features)
}

#[async_trait::async_trait]
impl Provider for Firecrawl {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: NAME,
            categories: &[
                Category::General,
                Category::News,
                Category::Science,
                Category::It,
            ],
            features: &[FeatureKind::News, FeatureKind::Images],
            renders: true,
            billable: true,
        }
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let mut body = json!({ "query": q.text, "limit": q.limit });
        body["sources"] = match q.category {
            Category::General => json!([{"type": "web"}, {"type": "news"}, {"type": "images"}]),
            Category::News => json!([{"type": "news"}]),
            Category::It | Category::Science => json!([{"type": "web"}]),
        };
        match q.category {
            Category::Science => body["categories"] = json!([{"type": "research"}]),
            Category::It => body["categories"] = json!([{"type": "developer"}]),
            Category::General | Category::News => {}
        }
        if let Some(r) = q.recent {
            body["tbs"] = json!(tbs(r));
        }
        if let Some(site) = &q.site {
            body["includeDomains"] = json!([site]);
        }
        let text = self.post(client, "/v2/search", &body).await?;
        let data = parse::<Envelope<SearchData>>(&text)?
            .data()?
            .unwrap_or_default();
        Ok(map(data, q.category))
    }

    async fn render(
        &self,
        client: &reqwest::Client,
        url: &url::Url,
    ) -> Result<Rendered, ProviderError> {
        let body = json!({ "url": url.as_str(), "formats": ["markdown"] });
        let text = self.post(client, "/v2/scrape", &body).await?;
        let data = parse::<Envelope<ScrapeData>>(&text)?
            .data()?
            .ok_or_else(|| ProviderError::Parse("scrape returned no data".into()))?;
        Ok(Rendered {
            url: non_empty(data.metadata.source_url).unwrap_or_else(|| url.to_string()),
            title: non_empty(data.metadata.title),
            markdown: data.markdown.unwrap_or_default(),
        })
    }
}
