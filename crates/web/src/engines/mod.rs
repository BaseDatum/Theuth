//! Keyless built-in engines (design doc 16 §3.2): Google, Bing, Brave and
//! Qwant SERP scrapes, plus Wikipedia, arXiv and GitHub. DuckDuckGo remains
//! constructible by name. [`builtin`] constructs one by its config name; the
//! small HTML/entity helpers every engine's parser needs live here.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::provider::{Provider, ProviderError};

pub mod arxiv;
pub mod bing;
pub mod brave;
pub mod duckduckgo;
pub mod github;
pub mod google;
pub mod qwant;
pub mod wikipedia;

pub use arxiv::Arxiv;
pub use bing::Bing;
pub use brave::Brave;
pub use duckduckgo::DuckDuckGo;
pub use github::Github;
pub use google::Google;
pub use qwant::Qwant;
pub use wikipedia::Wikipedia;

/// Default `web.engines` list. [`builtin`] also constructs `duckduckgo`.
pub const BUILTIN: &[&str] = &[
    "google",
    "bing",
    "brave",
    "qwant",
    "wikipedia",
    "arxiv",
    "github",
];

/// Identifiable user agent for APIs that require one (Wikimedia, GitHub).
pub const USER_AGENT: &str = "wth/0.1 (+https://github.com/skymoore/wth)";

/// Construct a built-in engine by name; `github` reads the optional
/// `github` credential. `None` for an unknown name.
#[must_use]
pub fn builtin(name: &str, credentials: &HashMap<String, String>) -> Option<Arc<dyn Provider>> {
    Some(match name {
        "google" => Arc::new(Google::default()),
        "bing" => Arc::new(Bing::default()),
        "brave" => Arc::new(Brave::default()),
        "qwant" => Arc::new(Qwant::default()),
        "duckduckgo" => Arc::new(DuckDuckGo::default()),
        "wikipedia" => Arc::new(Wikipedia::default()),
        "arxiv" => Arc::new(Arxiv::default()),
        "github" => Arc::new(Github::new(credentials.get("github").cloned())),
        _ => return None,
    })
}

/// Map a non-2xx status to the provider error the aggregator reports.
fn http_error(status: reqwest::StatusCode) -> ProviderError {
    match status.as_u16() {
        401 => ProviderError::Unauthorized,
        429 => ProviderError::RateLimited,
        403 => ProviderError::Blocked("HTTP 403".to_owned()),
        code => ProviderError::Http(format!("HTTP {code}")),
    }
}

/// Non-empty, trimmed string at a JSON position; `None` for anything else.
fn json_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Tags stripped, entities decoded, whitespace collapsed.
fn plain_text(html: &str) -> String {
    collapse_ws(&decode_entities(&strip_tags(html)))
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove `<…>` runs that look like tags (letter, `/`, `!` or `?` after
/// `<`); a bare `<` as in `a < b` survives.
fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        let is_tag = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        match after.find('>') {
            Some(gt) if is_tag => rest = &after[gt + 1..],
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decode the named and numeric entities that search APIs actually emit;
/// unknown `&…;` sequences are kept verbatim.
pub(crate) fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some(end) = rest.find(';')
            && let Some(ch) = decode_entity(&rest[..=end])
        {
            out.push(ch);
            rest = &rest[end + 1..];
            continue;
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "&amp;" => Some('&'),
        "&lt;" => Some('<'),
        "&gt;" => Some('>'),
        "&quot;" => Some('"'),
        "&apos;" | "&#39;" | "&#039;" => Some('\''),
        "&nbsp;" => Some(' '),
        _ => {
            let num = entity.strip_prefix("&#")?.strip_suffix(';')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_strips_tags_and_decodes() {
        assert_eq!(
            plain_text("<span class=\"searchmatch\">Rust</span> &amp; a &lt; b &#x27;x&#39;"),
            "Rust & a < b 'x'"
        );
        assert_eq!(plain_text("a < b &unknown; c"), "a < b &unknown; c");
    }
}
