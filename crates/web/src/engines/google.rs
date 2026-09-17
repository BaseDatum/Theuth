//! Google WEB (SearXNG-style WML scrape): organic results from
//! `GET {base}/wml/search` with a Nokia mobile user agent. No API key.
//! The parser is a tolerant class-token scan (`zMzFAb` rows, `fuLhoc`
//! links, `CVA68e` titles, `FrIlee` snippets, `ZWRArf` suggestions inside
//! the `HExoMb` table) rather than a DOM. `/url?q=` redirectors are
//! unwrapped; sorry/CAPTCHA pages are `Blocked`.

use std::collections::HashSet;

use reqwest::header::{ACCEPT, USER_AGENT};
use url::Url;

use crate::provider::{
    Category, Feature, FeatureItem, FeatureKind, Provider, ProviderError, ProviderMeta, Query,
    RawResult, Recency,
};

use super::{Elem, elements_with_class, http_url, reject_decoys};

/// Provider name.
pub const NAME: &str = "google";

/// Nokia user agents from SearXNG `nokia_useragents`: the WML endpoint only
/// serves the XML layout to these. One is picked per request.
pub const NOKIA_UAS: &[&str] = &[
    "Nokia7610/2.0 (5.0509.0) SymbianOS/7.0s Series60/2.1 Profile/MIDP-2.0 Configuration/CLDC-1.0",
    "Nokia7610/2.0 (7.0642.0) SymbianOS/7.0s Series60/2.1 Profile/MIDP-2.0 Configuration/CLDC-1.0",
    "Nokia6230/2.0 (05.50) Profile/MIDP-2.0 Configuration/CLDC-1.1",
    "Nokia6230i/2.0 (03.80) Profile/MIDP-2.0 Configuration/CLDC-1.1",
    "Nokia6280/2.0 (03.60) Profile/MIDP-2.0 Configuration/CLDC-1.1",
    "NokiaN72/2.0617.1.0.3 Series60/2.8 Profile/MIDP-2.0 Configuration/CLDC-1.1",
];

const RELATED_CAP: usize = 8;

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::General],
    features: &[FeatureKind::Related],
    renders: false,
    billable: false,
};

/// Google WEB engine. [`Default`] is `https://www.google.com`; [`Google::at`]
/// points `/wml/search` at one base for tests.
#[derive(Clone, Debug)]
pub struct Google {
    base: String,
}

impl Default for Google {
    fn default() -> Self {
        Self::at("https://www.google.com")
    }
}

impl Google {
    /// Serve `/wml/search` from `base`.
    #[must_use]
    pub fn at(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Google {
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
        let tbs = q.recent.map(tbs_filter);
        let mut params: Vec<(&str, &str)> = vec![
            ("q", text.as_str()),
            ("hl", "en"),
            ("lr", "lang_en"),
            ("ie", "utf8"),
            ("oe", "utf8"),
            ("sca_esv", "1"),
        ];
        if let Some(tbs) = tbs {
            params.push(("tbs", tbs));
        }
        let ua = NOKIA_UAS[rand::random_range(0..NOKIA_UAS.len())];
        let response = client
            .get(format!("{}/wml/search", self.base))
            .header(USER_AGENT, ua)
            .header(ACCEPT, "*/*")
            .query(&params)
            .send()
            .await?;
        let url = response.url().clone();
        let status = response.status().as_u16();
        let body = response.text().await?;
        let (results, features) = classify(&url, status, &body)?;
        let mut results = reject_decoys(results, &q.text)?;
        results.truncate(q.limit);
        Ok((results, features))
    }
}

fn tbs_filter(recent: Recency) -> &'static str {
    match recent {
        Recency::Day => "qdr:d",
        Recency::Week => "qdr:w",
        Recency::Month => "qdr:m",
        Recency::Year => "qdr:y",
    }
}

/// Status + final URL + body → parsed hits and suggestions, `Blocked`, or an
/// HTTP error. An empty SERP (2xx, zero rows) is `Ok(([], []))`. The shared
/// client follows redirects, so a sorry page shows up as the final URL
/// (`sorry.google.com` or `/sorry…`) or as a short stub linking `/sorry/`.
fn classify(
    url: &Url,
    status: u16,
    body: &str,
) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
    let sorry_url = url.host_str() == Some("sorry.google.com") || url.path().starts_with("/sorry");
    let sorry_stub = body.len() < 2000 && body.contains("/sorry/");
    if sorry_url || status == 403 || sorry_stub {
        return Err(ProviderError::Blocked(format!("HTTP {status} sorry")));
    }
    if !(200..300).contains(&status) {
        return Err(match status {
            401 => ProviderError::Unauthorized,
            429 => ProviderError::RateLimited,
            code => ProviderError::Http(format!("HTTP {code}")),
        });
    }
    Ok((parse_results(body), parse_related(body)))
}

/// Organic rows: each `zMzFAb` container's `fuLhoc` href, `CVA68e` title
/// and `FrIlee` snippet. Nested same-name tags can cut a container short,
/// so row bounds are consecutive container *starts*.
fn parse_results(body: &str) -> Vec<RawResult> {
    let containers = elements_with_class(body, "zMzFAb");
    let links = elements_with_class(body, "fuLhoc");
    let titles = elements_with_class(body, "CVA68e");
    let snippets = elements_with_class(body, "FrIlee");
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (i, container) in containers.iter().enumerate() {
        let row_start = container.start;
        let row_end = containers.get(i + 1).map_or(body.len(), |n| n.start);
        let in_row = |e: &Elem| e.start >= row_start && e.start < row_end;
        let Some(title) = titles.iter().find(|t| in_row(t)) else {
            continue;
        };
        if title.text.is_empty() {
            continue;
        }
        let Some(link) = links.iter().find(|l| in_row(l)) else {
            continue;
        };
        let Some(url) = link.href.as_deref().and_then(unwrap_google_url) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let snippet = snippets
            .iter()
            .find(|s| in_row(s))
            .map(|s| s.text.clone())
            .unwrap_or_default();
        out.push(RawResult {
            title: title.text.clone(),
            url,
            snippet,
            published: None,
            score_hint: None,
        });
    }
    out
}

/// Related searches: `ZWRArf` anchors inside a `HExoMb` table
/// (SearXNG `suggestion_xpath`), as one [`FeatureKind::Related`].
fn parse_related(body: &str) -> Vec<Feature> {
    let tables = elements_with_class(body, "HExoMb");
    if tables.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let items: Vec<FeatureItem> = elements_with_class(body, "ZWRArf")
        .into_iter()
        .filter(|a| tables.iter().any(|t| a.start > t.start && a.start < t.end))
        .filter(|a| !a.text.is_empty() && seen.insert(a.text.clone()))
        .take(RELATED_CAP)
        .map(|a| FeatureItem {
            title: a.text,
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

/// Unwrap `/url?q=` (unquote until `&sa=U`) and require an absolute
/// `http(s)` URL with a host. Bare relative paths are dropped.
fn unwrap_google_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let candidate = if let Some(rest) = raw.strip_prefix("/url?q=") {
        percent_decode(rest.split("&sa=U").next().unwrap_or(rest))
    } else {
        raw.to_owned()
    };
    http_url(&candidate)
}

fn percent_decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let raw = input.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Some(b) = hex_byte(raw[i + 1], raw[i + 2])
        {
            bytes.push(b);
            i += 3;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex_byte(h: u8, l: u8) -> Option<u8> {
    Some((hex_nibble(h)? << 4) | hex_nibble(l)?)
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn google_url() -> Url {
        Url::parse("https://www.google.com/wml/search").unwrap()
    }

    const SERP: &str = r#"<div class="zMzFAb">
  <a class="fuLhoc" href="/url?q=https%3A%2F%2Fexample.com%2Fpage%3Fid%3D1&amp;sa=U&amp;ved=0">
    <span class="CVA68e">Example Title</span>
  </a>
  <div class="taTFJ"><span class="FrIlee">A snippet about the &amp; page.</span></div>
</div>
<div class="zMzFAb">
  <a class="fuLhoc" href="https://crates.io/crates/async-trait">
    <span class="CVA68e">Direct Link</span>
  </a>
  <span class="FrIlee">No redirector.</span>
</div>
<table class="HExoMb"><tr><td>
  <a class="ZWRArf" href="/search?q=rust+book">rust book</a>
  <a class="ZWRArf" href="/search?q=rust+lang">rust lang</a>
</td></tr></table>
<a class="ZWRArf" href="/search?q=outside">outside table</a>"#;

    #[test]
    fn at_trims_slash_and_default_is_google() {
        assert_eq!(Google::default().base, "https://www.google.com");
        assert_eq!(Google::at("http://127.0.0.1:9/").base, "http://127.0.0.1:9");
        assert_eq!(META.features, &[FeatureKind::Related]);
    }

    #[test]
    fn parses_organic_rows_related_and_unwraps_redirect() {
        let (hits, features) = classify(&google_url(), 200, SERP).expect("serp");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Example Title");
        assert_eq!(hits[0].url, "https://example.com/page?id=1");
        assert_eq!(hits[0].snippet, "A snippet about the & page.");
        assert_eq!(hits[1].title, "Direct Link");
        assert_eq!(hits[1].url, "https://crates.io/crates/async-trait");
        assert_eq!(hits[1].snippet, "No redirector.");
        assert_eq!(features.len(), 1);
        assert_eq!(features[0].kind, FeatureKind::Related);
        let titles: Vec<&str> = features[0].items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["rust book", "rust lang"]);
    }

    #[test]
    fn empty_serp_is_ok() {
        let (hits, features) =
            classify(&google_url(), 200, "<html><body>no hits</body></html>").expect("empty");
        assert!(hits.is_empty());
        assert!(features.is_empty());
    }

    #[test]
    fn skips_row_without_title_or_http_url() {
        let body = r#"<div class="zMzFAb">
  <a class="fuLhoc" href="/url?q=https%3A%2F%2Fskip.example%2F"><span class="CVA68e"></span></a>
</div>
<div class="zMzFAb">
  <a class="fuLhoc" href="/relative"><span class="CVA68e">Rel</span></a>
</div>
<div class="zMzFAb">
  <a class="fuLhoc" href="/url?q=https%3A%2F%2Fok.example%2F">
    <span class="CVA68e">Kept</span>
  </a>
</div>"#;
        let hits = parse_results(body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Kept");
        assert_eq!(hits[0].url, "https://ok.example/");
    }

    #[test]
    fn unwrap_stops_at_sa_u() {
        assert_eq!(
            unwrap_google_url("/url?q=https%3A%2F%2Fexample.com%2Fa&sa=U&ved=x").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            unwrap_google_url("https://example.com/b").as_deref(),
            Some("https://example.com/b")
        );
        assert_eq!(unwrap_google_url("/y.js?q=rust"), None);
        assert_eq!(unwrap_google_url("javascript:void(0)"), None);
    }

    #[test]
    fn sorry_body_is_blocked() {
        let stub =
            "<html><a href=\"/sorry/index?continue=http://www.google.com/\">sorry</a></html>";
        assert!(stub.len() < 2000);
        assert!(matches!(
            classify(&google_url(), 200, stub),
            Err(ProviderError::Blocked(_))
        ));
    }

    #[test]
    fn sorry_host_and_path_are_blocked() {
        let host = Url::parse("https://sorry.google.com/sorry/index").unwrap();
        assert!(matches!(
            classify(&host, 200, "<html></html>"),
            Err(ProviderError::Blocked(_))
        ));
        let path = Url::parse("https://www.google.com/sorry/index?q=1").unwrap();
        assert!(matches!(
            classify(&path, 429, "<html></html>"),
            Err(ProviderError::Blocked(_))
        ));
    }

    #[test]
    fn http_403_is_blocked_and_429_is_rate_limited() {
        assert!(matches!(
            classify(&google_url(), 403, "forbidden"),
            Err(ProviderError::Blocked(_))
        ));
        assert!(matches!(
            classify(&google_url(), 429, "slow down"),
            Err(ProviderError::RateLimited)
        ));
    }

    #[test]
    fn long_body_mentioning_sorry_is_not_blocked() {
        let body = format!("{} /sorry/", "x".repeat(2000));
        let (hits, _) = classify(&google_url(), 200, &body).expect("long");
        assert!(hits.is_empty());
    }

    #[test]
    fn tbs_maps_recency() {
        assert_eq!(tbs_filter(Recency::Day), "qdr:d");
        assert_eq!(tbs_filter(Recency::Week), "qdr:w");
        assert_eq!(tbs_filter(Recency::Month), "qdr:m");
        assert_eq!(tbs_filter(Recency::Year), "qdr:y");
    }
}
