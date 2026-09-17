//! OpenSERP (16 §3.2): self-hosted browser-rendered Google/Bing/…,
//! `GET {base}/mega/search`; its `serp_features[]` are already structured,
//! so every module it classifies maps to a [`FeatureKind`]. Renders via
//! `GET {base}/extract?url=` (JSON `ExtractResult`). Hosted instances take
//! `Authorization: Bearer <key>`.

use serde::Deserialize;
use web::{
    Category, Feature, FeatureItem, FeatureKind, Link, Provider, ProviderError, ProviderMeta,
    Query, RawResult, Rendered,
};

use crate::{http_error, non_empty, parse};

const NAME: &str = "openserp";

/// OpenSERP provider against one server.
#[derive(Debug, Clone)]
pub struct Openserp {
    base: String,
    engines: String,
    key: Option<String>,
}

impl Openserp {
    /// `base` is the server root without a trailing slash; `engines` are
    /// upstream engine names for `/mega/search`; `key` only for the hosted API.
    #[must_use]
    pub fn new(base: &str, engines: &[String], key: Option<&str>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            engines: engines.join(","),
            key: key.map(str::to_string),
        }
    }

    async fn get(
        &self,
        client: &reqwest::Client,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<String, ProviderError> {
        let mut req = client.get(format!("{}{path}", self.base)).query(params);
        if let Some(k) = &self.key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await?;
        let status = resp.status();
        // 403/429 are the upstream engine blocking OpenSERP, not our key.
        if status.as_u16() == 403 {
            return Err(ProviderError::Blocked(
                "search engine blocked the request".into(),
            ));
        }
        if !status.is_success() {
            return Err(http_error(status));
        }
        Ok(resp.text().await?)
    }
}

/// `serp_features[].type` → our kind; `None` for modules we do not carry.
fn kind(t: &str) -> Option<FeatureKind> {
    Some(match t {
        "ai_summary" | "ai_overview" => FeatureKind::AiSummary,
        "answer_box" | "featured_snippet" => FeatureKind::Answer,
        "people_also_ask" | "faq" | "related_questions" => FeatureKind::Faq,
        "related_searches" => FeatureKind::Related,
        "top_stories" | "news" => FeatureKind::News,
        "knowledge_panel" | "knowledge_graph" => FeatureKind::KnowledgePanel,
        "dictionary" => FeatureKind::Definition,
        _ if t.starts_with("video") => FeatureKind::Videos,
        _ if t.starts_with("image") => FeatureKind::Images,
        _ => return None,
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    results: Vec<Hit>,
    serp_features: Vec<SerpFeature>,
    meta: Meta,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Meta {
    engines_failed: Vec<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    title: Option<String>,
    url: Option<String>,
    snippet: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SerpFeature {
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
    text: Option<String>,
    items: Vec<Item>,
    links: Vec<FeatureLink>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Item {
    title: Option<String>,
    text: Option<String>,
    link: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FeatureLink {
    title: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Extracted {
    url: Option<String>,
    title: Option<String>,
    markdown: Option<String>,
}

fn map(body: Response, limit: usize) -> (Vec<RawResult>, Vec<Feature>) {
    if !body.meta.engines_failed.is_empty() {
        tracing::debug!(engines = ?body.meta.engines_failed, "openserp: engines failed");
    }
    let results = body
        .results
        .into_iter()
        .filter_map(|h| {
            Some(RawResult {
                title: non_empty(h.title)?,
                url: non_empty(h.url)?,
                snippet: h.snippet.unwrap_or_default(),
                published: None,
                score_hint: None,
            })
        })
        .take(limit)
        .collect();
    let features = body
        .serp_features
        .into_iter()
        .filter_map(|f| {
            let Some(kind) = kind(&f.kind) else {
                tracing::debug!(kind = %f.kind, "openserp: unmapped serp feature");
                return None;
            };
            Some(Feature {
                kind,
                provider: NAME.to_string(),
                title: non_empty(f.title),
                text: non_empty(f.text),
                items: f
                    .items
                    .into_iter()
                    .filter_map(|i| {
                        let text = non_empty(i.text).unwrap_or_default();
                        let title = non_empty(i.title)
                            .or_else(|| (!text.is_empty()).then(|| text.clone()))?;
                        Some(FeatureItem {
                            title,
                            text,
                            url: non_empty(i.link),
                            date: None,
                        })
                    })
                    .collect(),
                links: f
                    .links
                    .into_iter()
                    .filter_map(|l| {
                        Some(Link {
                            url: non_empty(l.url)?,
                            title: non_empty(l.title).unwrap_or_default(),
                        })
                    })
                    .collect(),
                image: None,
                attributes: Vec::new(),
            })
        })
        .collect();
    (results, features)
}

#[async_trait::async_trait]
impl Provider for Openserp {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: NAME,
            categories: &[Category::General],
            features: &[
                FeatureKind::AiSummary,
                FeatureKind::Answer,
                FeatureKind::Faq,
                FeatureKind::Related,
                FeatureKind::News,
                FeatureKind::KnowledgePanel,
                FeatureKind::Definition,
                FeatureKind::Videos,
                FeatureKind::Images,
            ],
            renders: true,
            billable: false,
        }
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let limit = q.limit.to_string();
        let mut params = vec![
            ("engines", self.engines.as_str()),
            ("text", q.text.as_str()),
            ("limit", limit.as_str()),
        ];
        if let Some(site) = &q.site {
            params.push(("site", site.as_str()));
        }
        let body = self.get(client, "/mega/search", &params).await?;
        Ok(map(parse(&body)?, q.limit))
    }

    async fn render(
        &self,
        client: &reqwest::Client,
        url: &url::Url,
    ) -> Result<Rendered, ProviderError> {
        let body = self
            .get(
                client,
                "/extract",
                &[("url", url.as_str()), ("mode", "rendered")],
            )
            .await?;
        let e: Extracted = parse(&body)?;
        Ok(Rendered {
            url: non_empty(e.url).unwrap_or_else(|| url.to_string()),
            title: non_empty(e.title),
            markdown: e.markdown.unwrap_or_default(),
            tier: "render".into(),
        })
    }
}
