//! Keyless built-in engines (design doc 16 §3.2): Google, Bing, Brave, Qwant
//! and DuckDuckGo SERP scrapes, plus Wikipedia, arXiv and GitHub — five
//! general indexes so an out-of-box install still answers when some block
//! this network (2026-09-17: Google, Bing and Qwant did; Brave and DDG did
//! not). [`builtin`] constructs one by its config name; the small
//! HTML/entity helpers every engine's parser needs live here.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::provider::{Provider, ProviderError, RawResult};

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

/// Every name [`builtin`] accepts.
pub const BUILTIN: &[&str] = &[
    "google",
    "bing",
    "brave",
    "qwant",
    "duckduckgo",
    "wikipedia",
    "arxiv",
    "github",
];

/// Default `web.engines` list: [`BUILTIN`] minus `bing`, whose HTML endpoint
/// answers a flagged client with a genuine-looking "There are no results"
/// page (2026-09-27: 14 of 22 calls, 3 of 4 in a fresh probe). Opt back in
/// with `web.engines`.
pub const DEFAULT_ENGINES: &[&str] = &[
    "google",
    "brave",
    "qwant",
    "duckduckgo",
    "wikipedia",
    "arxiv",
    "github",
];

/// The general-web keyless engines; the degraded rule counts these
/// (Wikipedia, arXiv and GitHub are indexes of one kind of thing).
pub const KEYLESS_GENERAL: &[&str] = &["google", "bing", "brave", "qwant", "duckduckgo"];

/// Identifiable user agent for APIs that require one (Wikimedia, GitHub) and
/// the honest page-fetch identity under `impersonate: never` (122 D4).
pub fn user_agent() -> &'static str {
    static UA: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "theuth/{} (+https://github.com/skymoore/theuth)",
            platform::version::get()
        )
    });
    &UA
}

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

/// `Blocked` when a non-empty SERP shares no query word with any hit:
/// Bing (observed 2026-09-17) answers flagged clients with HTTP 200, the
/// right `<title>`, and ten cached results for some other query. Better a
/// reported block than confident garbage. Words under three chars and the
/// `site:` operator are ignored; one match anywhere clears the page.
fn reject_decoys(results: Vec<RawResult>, query: &str) -> Result<Vec<RawResult>, ProviderError> {
    let words: Vec<String> = query
        .split_whitespace()
        .filter(|w| !w.starts_with("site:"))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.chars().count() >= 3)
        .collect();
    if results.is_empty() || words.is_empty() {
        return Ok(results);
    }
    let relevant = results.iter().any(|r| {
        let hay = format!("{} {} {}", r.title, r.snippet, r.url).to_lowercase();
        words.iter().any(|w| hay.contains(w.as_str()))
    });
    if relevant {
        Ok(results)
    } else {
        Err(ProviderError::Blocked(
            "decoy results: no hit mentions the query".to_owned(),
        ))
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

/// One element found by class: byte span of the whole element, its `href`
/// (entity-decoded) and its plain text.
pub(crate) struct Elem {
    /// Offset of the opening `<`.
    pub(crate) start: usize,
    /// Offset just past the closing tag (or the self-closing `>`).
    pub(crate) end: usize,
    /// `href` attribute, entity-decoded.
    pub(crate) href: Option<String>,
    /// Inner plain text.
    pub(crate) text: String,
}

/// Every element whose `class` attribute contains the whole token `class`,
/// in document order. Tolerant: quotes, attribute order and nesting of
/// *other* tags do not matter; an element nesting its own tag name is cut
/// at the first closer.
pub(crate) fn elements_with_class(body: &str, class: &str) -> Vec<Elem> {
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
            end,
            href: attr(tag, "href").map(decode_entities),
            text: plain_text(inner),
        });
        from = end;
    }
    out
}

/// Quoted attribute value inside one tag; attribute order does not matter.
pub(crate) fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
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

/// Absolute `http(s)` URL with a host, normalised by the `url` crate.
pub(crate) fn http_url(s: &str) -> Option<String> {
    let parsed = url::Url::parse(s.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    Some(parsed.into())
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

    #[test]
    fn decoy_serp_is_blocked_relevant_one_passes() {
        let hit = |title: &str, url: &str| RawResult {
            title: title.into(),
            url: url.into(),
            ..RawResult::default()
        };
        let decoys = vec![
            hit("LinkedIn", "https://fr.linkedin.com/"),
            hit("知乎", "https://zhihu.com/q"),
        ];
        assert!(matches!(
            reject_decoys(decoys.clone(), "site:tokio.rs tokio async runtime"),
            Err(ProviderError::Blocked(_))
        ));
        // One hit mentioning one query word (in the URL is enough) clears the page.
        let mut ok = decoys;
        ok.push(hit("Home", "https://tokio.rs/"));
        assert_eq!(reject_decoys(ok, "tokio async runtime").unwrap().len(), 3);
        // Short words and an empty SERP never trigger it.
        assert!(reject_decoys(vec![hit("x", "https://a/")], "go io").is_ok());
        assert!(reject_decoys(Vec::new(), "anything").unwrap().is_empty());
    }
}
