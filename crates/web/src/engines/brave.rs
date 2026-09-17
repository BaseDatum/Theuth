//! Brave (design doc 16 §3.2): organic results from the keyless HTML
//! endpoint (`GET search.brave.com/search?q=&source=web`). Related queries
//! are lifted from `a.related-query` when present.
//!
//! The HTML endpoint 403s product user agents, so the request carries a
//! browser-like `User-Agent` plus the cookies SearXNG uses to pin region
//! and disable SafeSearch / location / summarizer.

use std::collections::HashSet;

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, COOKIE, USER_AGENT};
use url::Url;

use crate::provider::{
    Category, Feature, FeatureItem, FeatureKind, Provider, ProviderError, ProviderMeta, Query,
    RawResult, Recency,
};

use super::{decode_entities, plain_text};

/// Provider name.
pub const NAME: &str = "brave";

/// Browser-like user agent for the HTML endpoint (it 403s product UAs).
pub const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const ACCEPT_HTML: &str = "text/html,application/xhtml+xml";
const ACCEPT_LANG: &str = "en-US,en;q=0.9";
const COOKIES: &str = "safesearch=off; useLocation=0; summarizer=0; country=us; ui_lang=en-us";
const RELATED_CAP: usize = 8;

/// CAPTCHA / consent / sorry markers. Only decisive when the body has no
/// organic hits, so a snippet about a captcha is not a block.
const BLOCK_MARKERS: &[&str] = &["captcha", "sorry", "consent"];

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::General],
    features: &[FeatureKind::Related],
    renders: false,
    billable: false,
};

/// Brave web search. [`Default`] uses `https://search.brave.com`;
/// [`Brave::at`] points the same path at one base for tests.
#[derive(Clone, Debug)]
pub struct Brave {
    base: String,
}

impl Default for Brave {
    fn default() -> Self {
        Self {
            base: "https://search.brave.com".to_owned(),
        }
    }
}

impl Brave {
    /// `{base}/search` for tests.
    #[must_use]
    pub fn at(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Brave {
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
        let mut params: Vec<(&str, &str)> = vec![("q", text.as_str()), ("source", "web")];
        if let Some(tf) = q.recent.map(time_filter) {
            params.push(("tf", tf));
        }
        let response = client
            .get(format!("{}/search", self.base))
            .header(USER_AGENT, BROWSER_UA)
            .header(ACCEPT, ACCEPT_HTML)
            .header(ACCEPT_LANGUAGE, ACCEPT_LANG)
            .header(COOKIE, COOKIES)
            .query(&params)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        let (mut results, features) = classify(status, &body)?;
        results.truncate(q.limit);
        Ok((results, features))
    }
}

fn time_filter(recent: Recency) -> &'static str {
    match recent {
        Recency::Day => "pd",
        Recency::Week => "pw",
        Recency::Month => "pm",
        Recency::Year => "py",
    }
}

/// Status + body → parsed hits (and related), `Blocked`, or an HTTP error.
/// An empty SERP (2xx, zero rows, no captcha/sorry) is `Ok(([], _))`.
fn classify(
    status: reqwest::StatusCode,
    body: &str,
) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
    if !status.is_success() {
        return Err(super::http_error(status));
    }
    let results = parse_results(body);
    if results.is_empty()
        && let Some(marker) = block_marker(body)
    {
        return Err(ProviderError::Blocked(marker.to_owned()));
    }
    Ok((results, parse_related(body)))
}

fn block_marker(body: &str) -> Option<&'static str> {
    let lower = body.to_ascii_lowercase();
    BLOCK_MARKERS.iter().copied().find(|m| lower.contains(m))
}

/// Organic rows: each `div` whose class token is `snippet`, with an
/// absolute `http(s)` href (relative hrefs are ads), a `title` descendant,
/// and optional `content` descendant (not `site-name-content`).
/// Nested same-name tags can cut a container short, so row bounds are
/// consecutive snippet *starts*.
fn parse_results(body: &str) -> Vec<RawResult> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let snips = elements_with_class(body, "snippet");
    for (i, snip) in snips.iter().enumerate() {
        let row_end = snips.get(i + 1).map_or(body.len(), |n| n.start);
        let html = &body[snip.start..row_end];
        if !html.starts_with("<div") {
            continue;
        }
        let Some(href) = first_href(html) else {
            continue;
        };
        let Some(url) = http_url(&href) else {
            continue;
        };
        let title = elements_with_class(html, "title")
            .into_iter()
            .find(|e| !e.text.is_empty())
            .map(|e| e.text)
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }
        let snippet = elements_with_class(html, "content")
            .into_iter()
            .find(|e| !e.text.is_empty())
            .map(|e| e.text)
            .unwrap_or_default();
        out.push(RawResult {
            title,
            url,
            snippet,
            published: None,
            score_hint: None,
        });
    }
    out
}

fn parse_related(body: &str) -> Vec<Feature> {
    let items: Vec<FeatureItem> = elements_with_class(body, "related-query")
        .into_iter()
        .filter(|e| !e.text.is_empty())
        .take(RELATED_CAP)
        .map(|e| FeatureItem {
            title: e.text,
            ..Default::default()
        })
        .collect();
    if items.is_empty() {
        return Vec::new();
    }
    vec![Feature {
        kind: FeatureKind::Related,
        provider: NAME.to_owned(),
        items,
        ..Default::default()
    }]
}

/// First `href` on an `<a>` inside `html`, entity-decoded. Relative values
/// are returned as-is; the caller drops anything that is not absolute http(s).
fn first_href(html: &str) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = html[from..].find("<a") {
        let at = from + rel;
        let after = at + 2;
        let next = html[after..].chars().next();
        if next.is_some_and(|c| !c.is_whitespace() && !matches!(c, '>' | '/')) {
            from = after;
            continue;
        }
        let Some(gt) = html[at..].find('>') else {
            break;
        };
        let tag = &html[at..at + gt + 1];
        from = at + gt + 1;
        if let Some(href) = attr(tag, "href") {
            return Some(decode_entities(href));
        }
    }
    None
}

fn http_url(s: &str) -> Option<String> {
    let parsed = Url::parse(s.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    Some(parsed.into())
}

/// One element found by class: start byte and its plain text.
struct Elem {
    start: usize,
    text: String,
}

/// Every element whose `class` attribute contains the whole token `class`,
/// in document order. Tolerant: quotes, attribute order and nesting of
/// *other* tags do not matter; an element nesting its own tag name is cut
/// at the first closer.
fn elements_with_class(body: &str, class: &str) -> Vec<Elem> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = body[from..].find(class) {
        let at = from + rel;
        from = at + class.len();
        let Some(lt) = body[..at].rfind('<') else {
            continue;
        };
        if body[lt..at].contains('>') {
            continue; // text, not an attribute
        }
        let edge = |c: char| c.is_whitespace() || matches!(c, '"' | '\'');
        if !body[..at].ends_with(edge) || !body[from..].starts_with(edge) {
            continue; // `result__abc`, not `result__a`
        }
        let Some(gt) = body[at..].find('>') else {
            break;
        };
        let tag_end = at + gt + 1;
        let tag = &body[lt..tag_end];
        let name = tag[1..]
            .split(|c: char| c.is_whitespace() || matches!(c, '>' | '/'))
            .next()
            .unwrap_or_default();
        let (inner, end) = if tag.ends_with("/>") {
            ("", tag_end)
        } else {
            match body[tag_end..].find(&format!("</{name}")) {
                Some(c) => {
                    let inner_end = tag_end + c;
                    let end = body[inner_end..]
                        .find('>')
                        .map_or(body.len(), |g| inner_end + g + 1);
                    (&body[tag_end..inner_end], end)
                }
                None => ("", tag_end),
            }
        };
        out.push(Elem {
            start: lt,
            text: plain_text(inner),
        });
        from = end;
    }
    out
}

/// Quoted attribute value inside one tag; attribute order does not matter.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=");
    let mut from = 0;
    while let Some(rel) = tag[from..].find(&key) {
        let at = from + rel;
        from = at + key.len();
        if !tag[..at].ends_with(char::is_whitespace) {
            continue; // `data-href=`
        }
        let after = &tag[from..];
        let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let value = &after[1..];
        return Some(&value[..value.find(quote)?]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    const ORGANIC: &str = r#"
<div class="snippet fdb">
  <a href="https://example.com/page"><div class="title">Example Title</div></a>
  <div class="site-name-content">example.com</div>
  <div class="content">Example snippet text</div>
</div>
<div class="snippet fdb">
  <a href="/ad?x=1"><div class="title">Ad Title</div></a>
  <div class="content">skip relative</div>
</div>
<a class="related-query" href="/search?q=more">more like this</a>
"#;

    #[test]
    fn organic_yields_url_title_content_and_skips_relative() {
        let (hits, features) = classify(StatusCode::OK, ORGANIC).expect("serp");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/page");
        assert_eq!(hits[0].title, "Example Title");
        assert_eq!(hits[0].snippet, "Example snippet text");
        assert_eq!(features.len(), 1);
        assert_eq!(features[0].kind, FeatureKind::Related);
        assert_eq!(features[0].items[0].title, "more like this");
    }

    #[test]
    fn empty_serp_is_ok() {
        let (hits, features) =
            classify(StatusCode::OK, "<html><body>no hits</body></html>").expect("empty");
        assert!(hits.is_empty());
        assert!(features.is_empty());
    }

    #[test]
    fn captcha_and_sorry_block_when_no_organic() {
        assert!(matches!(
            classify(StatusCode::OK, "<html>Please complete this captcha</html>"),
            Err(ProviderError::Blocked(_))
        ));
        assert!(matches!(
            classify(StatusCode::OK, "<html>/sorry/ index</html>"),
            Err(ProviderError::Blocked(_))
        ));
        let serp = r#"<div class="snippet"><a href="https://e.com/"><div class="title">sorry captcha</div></a></div>"#;
        let (hits, _) = classify(StatusCode::OK, serp).expect("organic wins");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn forbidden_is_blocked() {
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, "nope"),
            Err(ProviderError::Blocked(_))
        ));
    }

    #[test]
    fn content_token_ignores_site_name_content() {
        let html = r#"<div class="snippet"><a href="https://ok.com/x"><span class="title">Ok</span></a><div class="site-name-content">not this</div></div>"#;
        let hits = parse_results(html);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.is_empty());
    }

    #[test]
    fn time_filter_maps_recency() {
        assert_eq!(time_filter(Recency::Day), "pd");
        assert_eq!(time_filter(Recency::Week), "pw");
        assert_eq!(time_filter(Recency::Month), "pm");
        assert_eq!(time_filter(Recency::Year), "py");
    }
}
