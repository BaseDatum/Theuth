//! Bing (design doc 16 §3.2): organic results from the keyless HTML
//! endpoint (`GET www.bing.com/search`). No Instant Answer / news JSON.
//!
//! The HTML endpoint 403s product user agents, so the request carries a
//! browser-like `User-Agent`. Organic rows are `li.b_algo`; click-tracking
//! `/ck/a?u=a1…` hrefs are base64url-decoded. An empty SERP is `Ok([])`.

use std::collections::HashSet;

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use url::Url;

use crate::provider::{Category, Feature, Provider, ProviderError, ProviderMeta, Query, RawResult};

use super::{decode_entities, plain_text};

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
        let status = response.status().as_u16();
        let body = response.text().await?;
        let mut results = classify(status, &body, &self.base)?;
        results.truncate(q.limit);
        Ok((results, Vec::new()))
    }
}

/// Status + body → parsed hits, `Blocked`, or an HTTP error. An empty SERP
/// (2xx, zero rows) is `Ok([])`.
fn classify(status: u16, body: &str, base: &str) -> Result<Vec<RawResult>, ProviderError> {
    if status == 403 {
        return Err(ProviderError::Blocked("HTTP 403".to_owned()));
    }
    if !(200..300).contains(&status) {
        return Err(ProviderError::Http(format!("HTTP {status}")));
    }
    let results = parse_results(body, base);
    if results.is_empty() && is_challenge(body) {
        return Err(ProviderError::Blocked("captcha/sorry".to_owned()));
    }
    Ok(results)
}

fn is_challenge(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("captcha") || lower.contains("sorry")
}

/// Organic rows: each `li` whose class contains the token `b_algo`.
fn parse_results(body: &str, base: &str) -> Vec<RawResult> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in elements_with_class(body, "b_algo") {
        let full = &body[item.start..item.end];
        if !tag_name(full).eq_ignore_ascii_case("li") {
            continue;
        }
        let inner = inner_html(full);
        let Some((title, href)) = title_and_href(inner) else {
            continue;
        };
        let Some(url) = resolve_url(&href, base) else {
            continue;
        };
        if title.is_empty() || !seen.insert(url.clone()) {
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

/// Title + href from the first `a` inside `h2`, else the first `a` with href.
fn title_and_href(inner: &str) -> Option<(String, String)> {
    first_element_inner(inner, "h2")
        .and_then(first_anchor)
        .or_else(|| first_anchor(inner))
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

/// Unwrap Bing `/ck/a?u=a1…` click-tracking and require an absolute http(s) URL.
fn resolve_url(href: &str, base: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    let absolute = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_owned()
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if href.starts_with('/') {
        format!("{base}{href}")
    } else {
        href.to_owned()
    };
    http_url(&unwrap_ck(&absolute, base))
}

fn unwrap_ck(href: &str, base: &str) -> String {
    let ck_here = format!("{base}/ck/a?");
    if !href.starts_with(&ck_here) && !href.starts_with("https://www.bing.com/ck/a?") {
        return href.to_owned();
    }
    let Ok(parsed) = Url::parse(href) else {
        return href.to_owned();
    };
    let Some(u) = parsed
        .query_pairs()
        .find_map(|(k, v)| (k == "u").then(|| v.into_owned()))
    else {
        return href.to_owned();
    };
    let Some(rest) = u.strip_prefix("a1") else {
        return href.to_owned();
    };
    match decode_base64url_utf8(rest) {
        Some(url) if !url.is_empty() => url,
        _ => href.to_owned(),
    }
}

fn http_url(s: &str) -> Option<String> {
    let parsed = Url::parse(s).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    Some(parsed.into())
}

fn decode_base64url_utf8(input: &str) -> Option<String> {
    let bytes = decode_base64url(input)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    fn digit(c: u8) -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        })
    }
    let chars: Vec<u8> = input
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
        .collect();
    if chars.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(chars.len() * 3 / 4);
    for chunk in chars.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let a = digit(chunk[0])?;
        let b = digit(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk.len() >= 3 {
            let c = digit(chunk[2])?;
            out.push(((b & 0x0f) << 4) | (c >> 2));
            if chunk.len() >= 4 {
                let d = digit(chunk[3])?;
                out.push(((c & 0x03) << 6) | d);
            }
        }
    }
    Some(out)
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

/// One element found by class: byte span of the matched tag.
struct Elem {
    start: usize,
    end: usize,
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
            continue;
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
        let end = if tag.ends_with("/>") {
            tag_end
        } else {
            match body[tag_end..].find(&format!("</{name}")) {
                Some(c) => {
                    let inner_end = tag_end + c;
                    body[inner_end..]
                        .find('>')
                        .map_or(body.len(), |g| inner_end + g + 1)
                }
                None => tag_end,
            }
        };
        out.push(Elem { start: lt, end });
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

    const TWO_HITS: &str = r#"<ol id="b_results">
<li class="b_algo">
<h2><a href="https://example.org/a">First title</a></h2>
<p>First snippet</p>
</li>
<li class="b_algo">
<h2><a href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS90d28">Second title</a></h2>
<p><span class="algoSlug_icon"></span>Second snippet</p>
</li>
</ol>"#;

    #[test]
    fn parse_two_b_algo_hits_and_ck_href() {
        let hits = classify(200, TWO_HITS, BING_HOST).expect("serp");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "First title");
        assert_eq!(hits[0].url, "https://example.org/a");
        assert_eq!(hits[0].snippet, "First snippet");
        assert_eq!(hits[1].title, "Second title");
        assert_eq!(hits[1].url, "https://example.com/two");
        assert_eq!(hits[1].snippet, "Second snippet");
    }

    #[test]
    fn ck_a1_base64url_decodes_against_at_base() {
        let html = r#"<ol id="b_results"><li class="b_algo">
<h2><a href="https://serp.test/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9kZWNvZGVk">T</a></h2>
<p>S</p>
</li></ol>"#;
        let hits = parse_results(html, "https://serp.test");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/decoded");
    }

    #[test]
    fn empty_serp_is_ok() {
        let hits = classify(200, r#"<ol id="b_results"></ol>"#, BING_HOST).expect("empty");
        assert!(hits.is_empty());
    }

    #[test]
    fn captcha_or_sorry_is_blocked() {
        assert!(matches!(
            classify(200, "<html>captcha</html>", BING_HOST),
            Err(ProviderError::Blocked(_))
        ));
        assert!(matches!(
            classify(200, "<html>Sorry</html>", BING_HOST),
            Err(ProviderError::Blocked(_))
        ));
    }

    #[test]
    fn http_403_is_blocked() {
        assert!(matches!(
            classify(403, TWO_HITS, BING_HOST),
            Err(ProviderError::Blocked(_))
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

    #[test]
    fn relative_ck_href_uses_engine_base() {
        let html = r#"<ol id="b_results"><li class="b_algo">
<h2><a href="/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS90d28">T</a></h2>
<p>S</p>
</li></ol>"#;
        let hits = parse_results(html, BING_HOST);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/two");
    }
}
