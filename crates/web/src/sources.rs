//! Known JSON sources (design doc 122 D11): hosts agents read most, read from
//! their public JSON endpoint instead of the HTML. Contract stub — no host
//! matches until W4 lands the eight-row table and its renderers.

/// A page URL that has a JSON endpoint worth reading instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiSource {
    /// Table key: `hn-item`, `reddit-old`, `npm`, `pypi`, `crates-io`, `arxiv`, `wikipedia`, `stackoverflow`.
    pub kind: &'static str,
    /// The URL to fetch instead of the page; always re-checked by the SSRF guard.
    pub fetch_url: url::Url,
}

/// The D11 table, matched on host and path; `None` for everything else.
///
/// Contract stub (122 §7.1): `None`.
#[must_use]
pub fn api_source(_url: &url::Url) -> Option<ApiSource> {
    None
}

/// Render a fetched body of `kind` as compact markdown; `None` when the body does not parse.
///
/// Contract stub (122 §7.1): `None`.
#[must_use]
pub fn render(_kind: &str, _body: &str) -> Option<String> {
    None
}
