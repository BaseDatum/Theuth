//! Qwant (design doc 16 §3.2): organic web results from the keyless JSON
//! API (`GET api.qwant.com/v3/search/web`). Web category only; ads and
//! other mainline types are dropped.

use reqwest::header::{ACCEPT, ORIGIN, REFERER};
use serde_json::Value;

use crate::provider::{Category, Feature, Provider, ProviderError, ProviderMeta, Query, RawResult};

use super::{json_str, plain_text};

/// Provider name.
pub const NAME: &str = "qwant";

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::General],
    features: &[],
    renders: false,
    billable: false,
};

/// Qwant web search. [`Default`] uses the real API; [`Qwant::at`] serves
/// `/web` from `base` for tests.
#[derive(Clone, Debug)]
pub struct Qwant {
    /// `GET {api}` — production is `https://api.qwant.com/v3/search/web`.
    api: String,
}

impl Default for Qwant {
    fn default() -> Self {
        Self::at("https://api.qwant.com/v3/search")
    }
}

impl Qwant {
    /// Serve `/web` from `base`.
    #[must_use]
    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            api: format!("{base}/web"),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Qwant {
    fn meta(&self) -> ProviderMeta {
        META
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let text = match &q.site {
            Some(site) => format!("site:{site} {}", q.text),
            None => q.text.clone(),
        };
        let count = q.limit.min(10).to_string();
        let response = client
            .get(&self.api)
            .header(ACCEPT, "application/json")
            .header(REFERER, "https://www.qwant.com/")
            .header(ORIGIN, "https://www.qwant.com")
            .query(&[
                ("q", text.as_str()),
                ("count", count.as_str()),
                ("locale", "en_US"),
                ("offset", "0"),
                ("device", "desktop"),
                ("safesearch", "0"),
                ("displayed", "true"),
                ("llm", "true"),
                ("tgp", "1"),
            ])
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        let mut results = parse_response(status, &body)?;
        results.truncate(q.limit);
        Ok((results, Vec::new()))
    }
}

/// Status + JSON body → web hits, `RateLimited`, `Blocked`, or an HTTP error.
/// An empty SERP (success, zero web rows) is `Ok([])`.
fn parse_response(status: u16, body: &str) -> Result<Vec<RawResult>, ProviderError> {
    match serde_json::from_str::<Value>(body) {
        Ok(doc) => classify(&doc, status),
        Err(e) => {
            if !(200..300).contains(&status) {
                return Err(http_status(status));
            }
            Err(ProviderError::Parse(e.to_string()))
        }
    }
}

fn classify(doc: &Value, status: u16) -> Result<Vec<RawResult>, ProviderError> {
    if doc["status"].as_str() != Some("success") {
        if error_code_is(&doc["data"]["error_code"], 24) {
            return Err(ProviderError::RateLimited);
        }
        if !doc["url"].is_null() {
            return Err(ProviderError::Blocked("captcha".to_owned()));
        }
        if status == 403 {
            return Err(ProviderError::Blocked("HTTP 403".to_owned()));
        }
        if !(200..300).contains(&status) {
            return Err(http_status(status));
        }
        return Err(ProviderError::Parse(api_error(&doc["data"])));
    }
    if !(200..300).contains(&status) {
        return Err(http_status(status));
    }
    Ok(web_results(doc))
}

fn error_code_is(v: &Value, code: u64) -> bool {
    v.as_u64() == Some(code)
        || v.as_i64() == Some(code as i64)
        || v.as_str().is_some_and(|s| s.trim() == code.to_string())
}

fn api_error(data: &Value) -> String {
    let msg = match &data["message"] {
        Value::Array(parts) => parts
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(","),
        Value::String(s) => s.clone(),
        _ => "unknown".to_owned(),
    };
    match &data["error_code"] {
        Value::Null => msg,
        code => format!("{msg} ({code})"),
    }
}

fn http_status(status: u16) -> ProviderError {
    reqwest::StatusCode::from_u16(status)
        .map(super::http_error)
        .unwrap_or_else(|_| ProviderError::Http(format!("HTTP {status}")))
}

/// `data.result.items.mainline[]` rows with `type == "web"`; ads and other
/// types are skipped. Items missing title or url are dropped.
fn web_results(doc: &Value) -> Vec<RawResult> {
    doc["data"]["result"]["items"]["mainline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["type"].as_str() == Some("web"))
        .flat_map(|row| {
            row["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    Some(RawResult {
                        title: json_str(&item["title"])?,
                        url: json_str(&item["url"])?,
                        snippet: item["desc"].as_str().map(plain_text).unwrap_or_default(),
                        published: None,
                        score_hint: None,
                    })
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS: &str = r#"{
        "status": "success",
        "data": {
            "result": {
                "items": {
                    "mainline": [
                        {
                            "type": "ads",
                            "items": [
                                {"title": "Buy now", "url": "https://ad.example/", "desc": "ad"}
                            ]
                        },
                        {
                            "type": "news",
                            "items": [
                                {"title": "Headline", "url": "https://news.example/", "desc": "n"}
                            ]
                        },
                        {
                            "type": "web",
                            "items": [
                                {
                                    "title": "Rust",
                                    "url": "https://www.rust-lang.org/",
                                    "desc": "A <b>language</b>."
                                },
                                {
                                    "title": "Cargo",
                                    "url": "https://doc.rust-lang.org/cargo/",
                                    "desc": "The package manager."
                                },
                                {"title": "", "url": "https://skip.example/", "desc": "no title"}
                            ]
                        }
                    ]
                }
            }
        }
    }"#;

    #[test]
    fn at_appends_web() {
        assert_eq!(
            Qwant::at("https://api.example/v3/search").api,
            "https://api.example/v3/search/web"
        );
        assert_eq!(Qwant::default().api, "https://api.qwant.com/v3/search/web");
    }

    #[test]
    fn parses_web_items_and_skips_ads() {
        let hits = parse_response(200, SUCCESS).expect("ok");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust");
        assert_eq!(hits[0].url, "https://www.rust-lang.org/");
        assert_eq!(hits[0].snippet, "A language.");
        assert_eq!(hits[1].title, "Cargo");
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/cargo/");
    }

    #[test]
    fn empty_serp_is_ok() {
        let body = r#"{"status":"success","data":{"result":{"items":{"mainline":[]}}}}"#;
        assert_eq!(parse_response(200, body).unwrap(), Vec::<RawResult>::new());
        let missing = r#"{"status":"success","data":{"result":{"items":{}}}}"#;
        assert_eq!(
            parse_response(200, missing).unwrap(),
            Vec::<RawResult>::new()
        );
    }

    #[test]
    fn error_code_24_is_rate_limited() {
        let body = r#"{"status":"error","data":{"error_code":24,"message":["slow down"]}}"#;
        assert!(matches!(
            parse_response(200, body),
            Err(ProviderError::RateLimited)
        ));
        let with_captcha =
            r#"{"status":"error","url":"https://www.qwant.com/captcha","data":{"error_code":24}}"#;
        assert!(matches!(
            parse_response(403, with_captcha),
            Err(ProviderError::RateLimited)
        ));
    }

    #[test]
    fn captcha_url_is_blocked() {
        let body = r#"{"status":"error","url":"https://www.qwant.com/captcha","data":{}}"#;
        assert!(matches!(
            parse_response(200, body),
            Err(ProviderError::Blocked(_))
        ));
    }

    #[test]
    fn http_403_is_blocked() {
        assert!(matches!(
            parse_response(403, "sorry"),
            Err(ProviderError::Blocked(_))
        ));
        let body = r#"{"status":"error","data":{"message":["denied"]}}"#;
        assert!(matches!(
            parse_response(403, body),
            Err(ProviderError::Blocked(_))
        ));
    }
}
