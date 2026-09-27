//! Bing (design doc 16 §3.2): organic results from the keyless HTML
//! endpoint (`GET www.bing.com/search`). No Instant Answer / news JSON.
//!
//! The HTML endpoint 403s product user agents, so the request carries a
//! browser-like `User-Agent`. Organic rows are `li.b_algo` with an `h2 > a`
//! link; click-tracking `/ck/a?u=a1…` hrefs are base64url-decoded by
//! [`crate::url::unwrap_bing_redirect`]. An empty SERP is `Ok([])`; only a
//! `/rewardsapp` or `/challenge` landing page, a `b_captcha` body or a 403
//! is `Blocked`. Bing has no time filter (SearXNG has none either), so a
//! recency query is `Unsupported`.

use std::collections::HashSet;

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use url::Url;

use crate::provider::{Category, Feature, Provider, ProviderError, ProviderMeta, Query, RawResult};

use super::{attr, decode_entities, elements_with_class, http_url, plain_text, reject_decoys};

/// Provider name.
pub const NAME: &str = "bing";

/// Browser-like user agent for the HTML endpoint (it 403s product UAs).
pub const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const ACCEPT_HTML: &str = "text/html,application/xhtml+xml";
const ACCEPT_LANG: &str = "en-US,en;q=0.9";
const BING_HOST: &str = "https://www.bing.com";

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::General],
    features: &[],
    renders: false,
    billable: false,
};

/// Bing engine. [`Default`] is `https://www.bing.com`; [`Bing::at`] points
/// the search path at one base for tests.
#[derive(Clone, Debug)]
pub struct Bing {
    base: String,
}

impl Default for Bing {
    fn default() -> Self {
        Self::at(BING_HOST)
    }
}

impl Bing {
    /// Serve `/search` from `base`.
    #[must_use]
    pub fn at(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Bing {
    fn meta(&self) -> ProviderMeta {
        META
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        if q.recent.is_some() {
            return Err(ProviderError::Unsupported);
        }
        let text = match &q.site {
            Some(site) => format!("site:{site} {}", q.text),
            None => q.text.clone(),
        };
        let response = client
            .get(format!("{}/search", self.base))
            .header(USER_AGENT, BROWSER_UA)
            .header(ACCEPT, ACCEPT_HTML)
            .header(ACCEPT_LANGUAGE, ACCEPT_LANG)
            .query(&[("q", text.as_str()), ("adlt", "off"), ("setlang", "en")])
            .send()
            .await?;
        let url = response.url().clone();
        let status = response.status().as_u16();
        let body = response.text().await?;
        let mut results = reject_decoys(classify(&url, status, &body)?, &q.text)?;
        results.truncate(q.limit);
        Ok((results, Vec::new()))
    }
}

/// Final URL + status + body → parsed hits, `Blocked`, or an HTTP error.
/// A `b_no` ("There are no results") page is `Blocked`: Bing serves it to
/// flagged clients for queries that plainly have results (2026-09-27), and
/// a genuine empty SERP for a real query is rarer than the flag.
fn classify(url: &Url, status: u16, body: &str) -> Result<Vec<RawResult>, ProviderError> {
    let path = url.path();
    if path.starts_with("/rewardsapp") || path.starts_with("/challenge") {
        return Err(ProviderError::Blocked(format!("landed on {path}")));
    }
    if body.contains("b_captcha") {
        return Err(ProviderError::Blocked("b_captcha".to_owned()));
    }
    if status == 403 {
        return Err(ProviderError::Blocked("HTTP 403".to_owned()));
    }
    if !(200..300).contains(&status) {
        return Err(ProviderError::Http(format!("HTTP {status}")));
    }
    let results = parse_results(body);
    if results.is_empty() && body.contains("class=\"b_no\"") {
        return Err(ProviderError::Blocked(
            "empty 'no results' page served to a flagged client".to_owned(),
        ));
    }
    Ok(results)
}

/// Organic rows: each `li` whose class contains the token `b_algo` and has
/// an `h2 > a` link. Rows without one (ads, sidebars) are skipped.
fn parse_results(body: &str) -> Vec<RawResult> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in elements_with_class(body, "b_algo") {
        let full = &body[item.start..item.end];
        if !tag_name(full).eq_ignore_ascii_case("li") {
            continue;
        }
        let inner = inner_html(full);
        let Some((title, href)) = first_element_inner(inner, "h2").and_then(first_anchor) else {
            continue;
        };
        let Some(url) = resolve_url(&href) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        out.push(RawResult {
            title,
            url,
            snippet: snippet(inner),
            published: None,
            score_hint: None,
        });
    }
    out
}

fn snippet(inner: &str) -> String {
    let mut from = 0;
    let mut parts = Vec::new();
    while let Some((tag_start, tag_end)) = find_open_tag(&inner[from..], "p") {
        let abs_end = from + tag_end;
        let tag = &inner[from + tag_start..abs_end];
        let text = plain_text(element_inner_after(tag, &inner[abs_end..], "p"));
        if !text.is_empty() {
            parts.push(text);
        }
        from = abs_end;
    }
    parts.join(" ")
}

/// First `<a>` with a non-blank `href` and non-empty text: `(text, href)`.
fn first_anchor(hay: &str) -> Option<(String, String)> {
    let mut from = 0;
    while let Some((tag_start, tag_end)) = find_open_tag(&hay[from..], "a") {
        let abs_start = from + tag_start;
        let abs_end = from + tag_end;
        let tag = &hay[abs_start..abs_end];
        let href = attr(tag, "href").map(decode_entities).unwrap_or_default();
        if href.trim().is_empty() {
            from = abs_end;
            continue;
        }
        let title = plain_text(element_inner_after(tag, &hay[abs_end..], "a"));
        if title.is_empty() {
            from = abs_end;
            continue;
        }
        return Some((title, href));
    }
    None
}

fn first_element_inner<'a>(hay: &'a str, name: &str) -> Option<&'a str> {
    let (tag_start, tag_end) = find_open_tag(hay, name)?;
    let tag = &hay[tag_start..tag_end];
    Some(element_inner_after(tag, &hay[tag_end..], name))
}

fn element_inner_after<'a>(tag: &str, after: &'a str, name: &str) -> &'a str {
    if tag.ends_with("/>") {
        return "";
    }
    match find_close_tag(after, name) {
        Some(c) => &after[..c],
        None => "",
    }
}

fn inner_html(full: &str) -> &str {
    let Some(gt) = full.find('>') else {
        return "";
    };
    if full[..gt + 1].ends_with("/>") {
        return "";
    }
    match full.rfind("</") {
        Some(c) if c > gt => &full[gt + 1..c],
        _ => &full[gt + 1..],
    }
}

fn tag_name(full: &str) -> &str {
    let rest = full.strip_prefix('<').unwrap_or(full);
    rest.split(|c: char| c.is_whitespace() || matches!(c, '>' | '/'))
        .next()
        .unwrap_or_default()
}

/// Resolve a SERP href against `www.bing.com`, unwrap `/ck/a?u=a1…` click
/// tracking, and require an absolute http(s) URL.
fn resolve_url(href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    let absolute = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if href.starts_with('/') {
        format!("{BING_HOST}{href}")
    } else {
        href.to_owned()
    };
    http_url(&crate::url::unwrap_bing_redirect(&absolute).unwrap_or(absolute))
}

fn find_open_tag(hay: &str, name: &str) -> Option<(usize, usize)> {
    let needle = format!("<{name}");
    let mut from = 0;
    while from < hay.len() {
        let rel = find_ci(&hay[from..], &needle)?;
        let start = from + rel;
        let after = start + needle.len();
        if tag_name_boundary(hay.as_bytes(), after) {
            let gt = hay[start..].find('>')?;
            return Some((start, start + gt + 1));
        }
        from = start + 1;
    }
    None
}

fn find_close_tag(hay: &str, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    let mut from = 0;
    while from < hay.len() {
        let rel = find_ci(&hay[from..], &needle)?;
        let start = from + rel;
        let after = start + needle.len();
        if hay
            .as_bytes()
            .get(after)
            .copied()
            .is_none_or(|c| c.is_ascii_whitespace() || c == b'>')
        {
            return Some(start);
        }
        from = start + 1;
    }
    None
}

fn tag_name_boundary(bytes: &[u8], after: usize) -> bool {
    bytes
        .get(after)
        .copied()
        .is_none_or(|c| c.is_ascii_whitespace() || matches!(c, b'>' | b'/'))
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bing_url() -> Url {
        Url::parse("https://www.bing.com/search?q=rust").unwrap()
    }

    const TWO_HITS: &str = r#"<ol id="b_results">
<li class="b_algo">
<h2><a href="https://example.org/a">First title</a></h2>
<p>First snippet</p>
</li>
<li class="b_algo">
<div class="b_tpcn"><a class="tilk" href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS90d28">example.com</a></div>
<h2><a href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS90d28">Second title</a></h2>
<p><span class="algoSlug_icon"></span>Second snippet</p>
</li>
<li class="b_algo b_ad">
<a href="https://ads.example/">Sidebar ad without h2</a>
</li>
</ol>"#;

    #[test]
    fn parse_h2_hits_unwrap_ck_and_skip_rows_without_h2() {
        let hits = classify(&bing_url(), 200, TWO_HITS).expect("serp");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "First title");
        assert_eq!(hits[0].url, "https://example.org/a");
        assert_eq!(hits[0].snippet, "First snippet");
        assert_eq!(hits[1].title, "Second title");
        assert_eq!(hits[1].url, "https://example.com/two");
        assert_eq!(hits[1].snippet, "Second snippet");
    }

    #[test]
    fn relative_ck_href_resolves_against_bing_host() {
        let html = r#"<ol id="b_results"><li class="b_algo">
<h2><a href="/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS90d28">T</a></h2>
<p>S</p>
</li></ol>"#;
        let hits = parse_results(html);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/two");
    }

    #[test]
    fn empty_serp_is_ok_even_when_it_says_sorry() {
        assert!(
            classify(&bing_url(), 200, r#"<ol id="b_results"></ol>"#)
                .expect("empty")
                .is_empty()
        );
        assert!(
            classify(
                &bing_url(),
                200,
                "<html>Sorry, there are no results for captcha</html>"
            )
            .expect("no results")
            .is_empty()
        );
    }

    #[test]
    fn captcha_id_and_challenge_paths_are_blocked() {
        assert!(matches!(
            classify(&bing_url(), 200, r#"<div id="b_captcha"></div>"#),
            Err(ProviderError::Blocked(_))
        ));
        for landing in [
            "https://www.bing.com/rewardsapp/flyout",
            "https://www.bing.com/challenge?x=1",
        ] {
            let url = Url::parse(landing).unwrap();
            assert!(
                matches!(
                    classify(&url, 200, TWO_HITS),
                    Err(ProviderError::Blocked(_))
                ),
                "{landing}"
            );
        }
    }

    #[test]
    fn http_403_is_blocked_and_500_is_http() {
        assert!(matches!(
            classify(&bing_url(), 403, TWO_HITS),
            Err(ProviderError::Blocked(_))
        ));
        assert!(matches!(
            classify(&bing_url(), 500, ""),
            Err(ProviderError::Http(_))
        ));
    }

    #[test]
    fn default_and_at_trim_slash() {
        assert_eq!(Bing::default().base, BING_HOST);
        assert_eq!(Bing::at("https://serp.test/").base, "https://serp.test");
        assert_eq!(META.name, "bing");
        assert_eq!(META.categories, &[Category::General]);
        assert!(META.features.is_empty());
        const {
            assert!(!META.renders);
            assert!(!META.billable);
        }
    }
}
