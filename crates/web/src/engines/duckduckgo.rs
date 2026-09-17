//! DuckDuckGo (design doc 16 §3.2): organic results from the HTML endpoint
//! (`POST html.duckduckgo.com/html/`, lite fallback when blocked) and SERP
//! features from the keyless Instant Answer API (`api.duckduckgo.com`).
//!
//! The HTML endpoints 403 product user agents, so those two requests carry
//! a browser-like `User-Agent`; the JSON API gets the shared client's.
//! The organic parser is a tolerant scan over the known markup
//! (`class="result__a"` links, `result__snippet` text, `result--ad` rows)
//! rather than a DOM: DDG's SERP is server-rendered and stable, and the
//! `Blocked` classification, not the selector, is what fails in practice.

use std::collections::HashSet;

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, REFERER, USER_AGENT};
use serde_json::Value;
use url::Url;

use crate::provider::{
    Category, Feature, FeatureItem, FeatureKind, Link, Provider, ProviderError, ProviderMeta,
    Query, RawResult, Recency,
};

use super::{elements_with_class, http_url, json_str};

/// Provider name.
pub const NAME: &str = "duckduckgo";

/// Browser-like user agent for the HTML endpoints (they 403 product UAs).
pub const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const ACCEPT_HTML: &str = "text/html,application/xhtml+xml";
const ACCEPT_LANG: &str = "en-US,en;q=0.9";
const RELATED_CAP: usize = 8;

/// Bot wall / CAPTCHA / Cloudflare interstitial markers. Only decisive when
/// the body has no organic links, so a snippet about Cloudflare is not a block.
const CHALLENGE_MARKERS: &[&str] = &[
    "anomaly-modal",
    "Please complete the following challenge",
    "cf-browser-verification",
    "challenge-platform",
    "Attention Required! | Cloudflare",
    "Just a moment...",
    "Enable JavaScript and cookies to continue",
];

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::General, Category::News],
    features: &[
        FeatureKind::Answer,
        FeatureKind::KnowledgePanel,
        FeatureKind::Definition,
        FeatureKind::Related,
    ],
    renders: false,
    billable: false,
};

/// DuckDuckGo engine. [`Default`] uses the real hosts; [`DuckDuckGo::at`]
/// points every endpoint at one base for tests.
#[derive(Clone, Debug)]
pub struct DuckDuckGo {
    /// Host serving `/html/`.
    html: String,
    /// Host serving `/lite/`.
    lite: String,
    /// Instant Answer API host (`/?q=…&format=json`).
    api: String,
}

impl Default for DuckDuckGo {
    fn default() -> Self {
        Self {
            html: "https://html.duckduckgo.com".to_owned(),
            lite: "https://lite.duckduckgo.com".to_owned(),
            api: "https://api.duckduckgo.com".to_owned(),
        }
    }
}

impl DuckDuckGo {
    /// All three endpoints on `base`: `{base}/html/`, `{base}/lite/`, `{base}/`.
    #[must_use]
    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_owned();
        Self {
            html: base.clone(),
            lite: base.clone(),
            api: base,
        }
    }

    async fn organic(
        &self,
        client: &reqwest::Client,
        text: &str,
        recent: Option<Recency>,
    ) -> Result<Vec<RawResult>, ProviderError> {
        let mut form = vec![("q", text)];
        if let Some(df) = recent.map(date_filter) {
            form.push(("df", df));
        }
        let response = client
            .post(format!("{}/html/", self.html))
            .header(USER_AGENT, BROWSER_UA)
            .header(ACCEPT, ACCEPT_HTML)
            .header(ACCEPT_LANGUAGE, ACCEPT_LANG)
            .header(REFERER, "https://html.duckduckgo.com/")
            .form(&form)
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        match classify(status, &body, "result__a", "result__snippet") {
            Err(ProviderError::Blocked(reason)) => {
                tracing::debug!(reason = %reason, "duckduckgo html blocked; trying lite");
            }
            done => return done,
        }

        let response = client
            .get(format!("{}/lite/", self.lite))
            .header(USER_AGENT, BROWSER_UA)
            .header(ACCEPT, ACCEPT_HTML)
            .header(ACCEPT_LANGUAGE, ACCEPT_LANG)
            .query(&form)
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        classify(status, &body, "result-link", "result-snippet")
    }

    async fn instant_answers(
        &self,
        client: &reqwest::Client,
        text: &str,
    ) -> Result<Vec<Feature>, ProviderError> {
        let response = client
            .get(format!("{}/", self.api))
            .query(&[
                ("q", text),
                ("format", "json"),
                ("no_html", "1"),
                ("skip_disambig", "1"),
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
        Ok(features(&doc))
    }
}

#[async_trait::async_trait]
impl Provider for DuckDuckGo {
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
        let (results, features) = tokio::join!(
            self.organic(client, &text, q.recent),
            self.instant_answers(client, &q.text)
        );
        let mut results = results?;
        results.truncate(q.limit);
        let features = features.unwrap_or_else(|e| {
            tracing::debug!(error = %e, "duckduckgo instant answers failed");
            Vec::new()
        });
        Ok((results, features))
    }
}

fn date_filter(recent: Recency) -> &'static str {
    match recent {
        Recency::Day => "d",
        Recency::Week => "w",
        Recency::Month => "m",
        Recency::Year => "y",
    }
}

/// Status + body → parsed hits, `Blocked`, or an HTTP error. An empty SERP
/// (2xx, zero rows) is `Ok([])`.
fn classify(
    status: u16,
    body: &str,
    link_class: &str,
    snippet_class: &str,
) -> Result<Vec<RawResult>, ProviderError> {
    let has_organic = || {
        elements_with_class(body, link_class)
            .iter()
            .any(|e| e.href.as_deref().is_some_and(|h| !h.trim().is_empty()))
    };
    // DDG's 403 is a short "email us" page without any challenge marker.
    if status == 403
        || (CHALLENGE_MARKERS.iter().any(|m| body.contains(m)) && !has_organic())
        || (status == 202 && !has_organic())
    {
        return Err(ProviderError::Blocked(format!("HTTP {status} challenge")));
    }
    if !(200..300).contains(&status) {
        return Err(ProviderError::Http(format!("HTTP {status}")));
    }
    let mut results = parse_results(body, link_class, snippet_class);
    if results.is_empty() && link_class != "result-link" {
        // The classic host sometimes serves the lite layout.
        results = parse_results(body, "result-link", "result-snippet");
    }
    Ok(results)
}

/// Organic rows: each link, its snippet (the first snippet element before
/// the next link), minus ad rows (an opening tag between the previous row's
/// end and this link whose attributes carry `result--ad`; comments and text
/// do not count) and duplicate URLs.
fn parse_results(body: &str, link_class: &str, snippet_class: &str) -> Vec<RawResult> {
    let links = elements_with_class(body, link_class);
    let snippets = elements_with_class(body, snippet_class);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut row_start = 0;
    for (i, link) in links.iter().enumerate() {
        let next = links.get(i + 1).map_or(body.len(), |l| l.start);
        let snippet = snippets
            .iter()
            .find(|s| s.start >= link.end && s.start < next);
        let is_ad = body[row_start..link.start]
            .split('<')
            .filter(|t| !t.starts_with('!'))
            .map(|t| &t[..t.find('>').unwrap_or(t.len())])
            .any(|tag| tag.contains("result--ad"));
        row_start = snippet.map_or(link.end, |s| s.end.max(link.end));
        if is_ad || link.text.is_empty() {
            continue;
        }
        let Some(url) = link.href.as_deref().and_then(resolve_url) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        out.push(RawResult {
            title: link.text.clone(),
            url,
            snippet: snippet.map(|s| s.text.clone()).unwrap_or_default(),
            published: None,
            score_hint: None,
        });
    }
    out
}

/// Unwrap the `//duckduckgo.com/l/?uddg=…` redirect and require an
/// absolute `http(s)` URL with a host. Bare relative paths are dropped.
fn resolve_url(href: &str) -> Option<String> {
    let href = href.trim();
    let candidate = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{href}")
    } else {
        href.to_owned()
    };
    let parsed = Url::parse(&candidate).ok()?;
    let uddg = parsed
        .query_pairs()
        .find_map(|(k, v)| (k == "uddg" && !v.is_empty()).then(|| v.into_owned()));
    match uddg {
        Some(dest) => http_url(&dest),
        None if href.starts_with('/') && !href.starts_with("//") => None,
        None => http_url(&candidate),
    }
}

/// SERP features from one Instant Answer document (`format=json&no_html=1`).
#[must_use]
pub fn features(doc: &Value) -> Vec<Feature> {
    let heading = json_str(&doc["Heading"]);
    let feature = |kind| Feature {
        kind,
        provider: NAME.to_owned(),
        title: heading.clone(),
        ..Feature::default()
    };
    let source_link = |source: &str, url: &str| -> Vec<Link> {
        json_str(&doc[url])
            .map(|url| Link {
                title: json_str(&doc[source]).unwrap_or_default(),
                url,
            })
            .into_iter()
            .collect()
    };

    let mut out = Vec::new();
    if let Some(answer) = json_str(&doc["Answer"]) {
        out.push(Feature {
            text: Some(answer),
            ..feature(FeatureKind::Answer)
        });
    }
    if let Some(text) = json_str(&doc["AbstractText"]).or_else(|| json_str(&doc["Abstract"])) {
        let image = json_str(&doc["Image"]).map(|i| {
            if i.starts_with('/') {
                format!("https://duckduckgo.com{i}")
            } else {
                i
            }
        });
        let attributes = doc["Infobox"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((json_str(&row["label"])?, scalar(&row["value"])?)))
            .collect();
        out.push(Feature {
            text: Some(text),
            links: source_link("AbstractSource", "AbstractURL"),
            image,
            attributes,
            ..feature(FeatureKind::KnowledgePanel)
        });
    }
    if let Some(definition) = json_str(&doc["Definition"]) {
        out.push(Feature {
            text: Some(definition),
            links: source_link("DefinitionSource", "DefinitionURL"),
            ..feature(FeatureKind::Definition)
        });
    }
    let mut items = Vec::new();
    related(&doc["RelatedTopics"], &mut items);
    if !items.is_empty() {
        out.push(Feature {
            title: None,
            items,
            ..feature(FeatureKind::Related)
        });
    }
    out
}

/// Flatten `RelatedTopics` (plain entries and `Topics` groups), capped.
fn related(topics: &Value, items: &mut Vec<FeatureItem>) {
    for topic in topics.as_array().into_iter().flatten() {
        if items.len() >= RELATED_CAP {
            return;
        }
        match json_str(&topic["Text"]) {
            Some(title) => items.push(FeatureItem {
                title,
                url: json_str(&topic["FirstURL"]),
                ..FeatureItem::default()
            }),
            None => related(&topic["Topics"], items),
        }
    }
}

/// Infobox values are strings or numbers; structured rows are skipped.
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        _ => json_str(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_unwraps_redirects_and_rejects_junk() {
        assert_eq!(
            resolve_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=x").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            resolve_url("https://crates.io/crates/async-trait").as_deref(),
            Some("https://crates.io/crates/async-trait")
        );
        assert_eq!(resolve_url("/y.js?q=rust"), None);
        assert_eq!(resolve_url("javascript:void(0)"), None);
    }

    #[test]
    fn class_token_must_be_whole_and_inside_a_tag() {
        let body = r#"<p>result__a</p><a class="result__abc" href="/x">no</a><a class="result__a" href="https://e.com/">yes</a>"#;
        let found = elements_with_class(body, "result__a");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "yes");
        assert_eq!(found[0].href.as_deref(), Some("https://e.com/"));
    }

    #[test]
    fn challenge_markers_do_not_block_a_real_serp() {
        let serp = r#"<div class="result"><a class="result__a" href="https://e.com/">Just a moment...</a></div>"#;
        let hits = classify(200, serp, "result__a", "result__snippet").expect("serp");
        assert_eq!(hits.len(), 1);
        assert!(matches!(
            classify(
                202,
                "<html>Just a moment...</html>",
                "result__a",
                "result__snippet"
            ),
            Err(ProviderError::Blocked(_))
        ));
    }
}
