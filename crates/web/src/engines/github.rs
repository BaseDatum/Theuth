//! GitHub (design doc 16 §3.2): repository search only
//! (`GET api.github.com/search/repositories`), never code search, to spare
//! the tight Search API quota (~10 req/min anonymous, ~30 with a token).
//! GitHub signals an exhausted quota as **403** with a rate-limit body or
//! `x-ratelimit-remaining: 0`, not 429; both map to `RateLimited`.
//! `site` and `recent` are ignored (every hit is on github.com; the date
//! qualifier would need a calendar).

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, USER_AGENT};
use serde_json::Value;

use crate::provider::{Category, Feature, Provider, ProviderError, ProviderMeta, Query, RawResult};

use super::json_str;

/// Provider name; also the optional credential key.
pub const NAME: &str = "github";

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::It],
    features: &[],
    renders: false,
    billable: false,
};

/// GitHub repository search, optionally authenticated.
#[derive(Clone, Debug)]
pub struct Github {
    base: String,
    token: Option<String>,
}

impl Github {
    /// Production host `https://api.github.com`; `token` raises the quota.
    #[must_use]
    pub fn new(token: Option<String>) -> Self {
        Self::at("https://api.github.com", token)
    }

    /// Serve `/search/repositories` from `base`.
    #[must_use]
    pub fn at(base: &str, token: Option<String>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            token: token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Github {
    fn meta(&self) -> ProviderMeta {
        META
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let per_page = q.limit.to_string();
        let mut request = client
            .get(format!("{}/search/repositories", self.base))
            .header(USER_AGENT, super::user_agent())
            .header(ACCEPT, "application/vnd.github+json")
            .query(&[("q", q.text.as_str()), ("per_page", per_page.as_str())]);
        if let Some(token) = &self.token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let body = response.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                403 if rate_limited(&headers, &body) => ProviderError::RateLimited,
                _ => super::http_error(status),
            });
        }
        let doc: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        Ok((results(&doc), Vec::new()))
    }
}

/// A 403 is a quota failure when the remaining count is zero or the body
/// says so (secondary limits sometimes omit the header).
fn rate_limited(headers: &HeaderMap, body: &str) -> bool {
    let remaining_zero = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.trim() == "0");
    let lower = body.to_ascii_lowercase();
    remaining_zero
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("abuse detection")
}

/// `items[]` → results; `stargazers_count` becomes the score hint.
fn results(doc: &Value) -> Vec<RawResult> {
    doc["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(RawResult {
                title: json_str(&item["full_name"])?,
                url: json_str(&item["html_url"])?,
                snippet: json_str(&item["description"]).unwrap_or_default(),
                published: None,
                score_hint: item["stargazers_count"].as_f64(),
            })
        })
        .collect()
}
