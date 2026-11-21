//! Wikipedia (design doc 16 §3.2): MediaWiki `list=search` for results and
//! the REST `page/summary/{title}` of the top hit as one `KnowledgePanel`.
//! Both requests carry the identifiable user agent Wikimedia requires.
//! `published` is never set: MediaWiki's `timestamp` is the last revision,
//! not a publication date.

use std::fmt::Write as _;

use reqwest::header::USER_AGENT;
use serde_json::Value;

use crate::provider::{
    Category, Feature, FeatureKind, Link, Provider, ProviderError, ProviderMeta, Query, RawResult,
};

use super::{json_str, plain_text};

/// Provider name.
pub const NAME: &str = "wikipedia";

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::General, Category::Science],
    features: &[FeatureKind::KnowledgePanel],
    renders: false,
    billable: false,
};

/// English Wikipedia. [`Default`] is `https://en.wikipedia.org`.
#[derive(Clone, Debug)]
pub struct Wikipedia {
    base: String,
}

impl Default for Wikipedia {
    fn default() -> Self {
        Self::at("https://en.wikipedia.org")
    }
}

impl Wikipedia {
    /// Serve `/w/api.php`, `/api/rest_v1/…` and `/wiki/…` from `base`.
    #[must_use]
    pub fn at(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    async fn summary(
        &self,
        client: &reqwest::Client,
        title: &str,
    ) -> Result<Option<Feature>, ProviderError> {
        let response = client
            .get(format!(
                "{}/api/rest_v1/page/summary/{}",
                self.base,
                encode_title(title)
            ))
            .header(USER_AGENT, super::user_agent())
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(super::http_error(response.status()));
        }
        let doc: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        Ok(knowledge_panel(&doc))
    }
}

#[async_trait::async_trait]
impl Provider for Wikipedia {
    fn meta(&self) -> ProviderMeta {
        META
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let limit = q.limit.to_string();
        let response = client
            .get(format!("{}/w/api.php", self.base))
            .header(USER_AGENT, super::user_agent())
            .query(&[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", q.text.as_str()),
                ("srlimit", limit.as_str()),
                ("format", "json"),
                ("utf8", "1"),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(super::http_error(response.status()));
        }
        let doc: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        if let Some(err) = doc.get("error") {
            return Err(ProviderError::Parse(format!(
                "{}: {}",
                json_str(&err["code"]).unwrap_or_else(|| "error".to_owned()),
                json_str(&err["info"]).unwrap_or_default()
            )));
        }
        let results: Vec<RawResult> = doc["query"]["search"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|hit| {
                let title = json_str(&hit["title"])?;
                Some(RawResult {
                    url: format!("{}/wiki/{}", self.base, encode_title(&title)),
                    title,
                    snippet: plain_text(hit["snippet"].as_str().unwrap_or_default()),
                    published: None,
                    score_hint: None,
                })
            })
            .collect();

        let features = match results.first() {
            Some(top) => self.summary(client, &top.title).await.unwrap_or_else(|e| {
                tracing::debug!(error = %e, "wikipedia summary failed");
                None
            }),
            None => None,
        };
        Ok((results, features.into_iter().collect()))
    }
}

/// One `KnowledgePanel` from a REST `page/summary` document; `None` for
/// disambiguation pages and empty extracts.
#[must_use]
pub fn knowledge_panel(doc: &Value) -> Option<Feature> {
    if doc["type"].as_str() == Some("disambiguation") {
        return None;
    }
    let extract = json_str(&doc["extract"])?;
    Some(Feature {
        kind: FeatureKind::KnowledgePanel,
        provider: NAME.to_owned(),
        title: json_str(&doc["title"]),
        text: Some(extract),
        items: Vec::new(),
        links: json_str(&doc["content_urls"]["desktop"]["page"])
            .map(|url| Link {
                title: "Wikipedia".to_owned(),
                url,
            })
            .into_iter()
            .collect(),
        image: json_str(&doc["thumbnail"]["source"]),
        attributes: json_str(&doc["description"])
            .map(|d| ("description".to_owned(), d))
            .into_iter()
            .collect(),
    })
}

/// Article title as one path segment: spaces to `_`, everything outside the
/// unreserved and MediaWiki-friendly set percent-encoded (UTF-8 bytes).
fn encode_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len() * 2);
    for &b in title.replace(' ', "_").as_bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'('
            | b')'
            | b','
            | b':'
            | b'\''
            | b'*'
            | b'!'
            | b'+'
            | b'='
            | b'@'
            | b'$'
            | b';' => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_encoding_keeps_parens_and_escapes_the_rest() {
        assert_eq!(
            encode_title("Rust (programming language)"),
            "Rust_(programming_language)"
        );
        assert_eq!(encode_title("AC/DC"), "AC%2FDC");
        assert_eq!(encode_title("Ada Lovelace?"), "Ada_Lovelace%3F");
        assert_eq!(encode_title("Zürich"), "Z%C3%BCrich");
    }
}
