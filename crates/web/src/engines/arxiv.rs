//! arXiv (design doc 16 §3.2): the Atom API at `export.arxiv.org/api/query`,
//! read with a hand-rolled entry scanner instead of an XML crate. It assumes
//! arXiv-shaped Atom: flat `<entry>` elements, entity-encoded text, no
//! CDATA, no nested same-name tags. `site` and `recent` are ignored: the
//! API has no date filter and every result is on arxiv.org.

use crate::provider::{Category, Feature, Provider, ProviderError, ProviderMeta, Query, RawResult};

use super::{collapse_ws, decode_entities};

/// Provider name.
pub const NAME: &str = "arxiv";

const META: ProviderMeta = ProviderMeta {
    name: NAME,
    categories: &[Category::Science],
    features: &[],
    renders: false,
    billable: false,
};

/// arXiv engine. [`Default`] is `https://export.arxiv.org`.
#[derive(Clone, Debug)]
pub struct Arxiv {
    base: String,
}

impl Default for Arxiv {
    fn default() -> Self {
        Self::at("https://export.arxiv.org")
    }
}

impl Arxiv {
    /// Serve `/api/query` from `base`.
    #[must_use]
    pub fn at(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Arxiv {
    fn meta(&self) -> ProviderMeta {
        META
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
        let search_query = format!("all:{}", q.text);
        let max_results = q.limit.to_string();
        let response = client
            .get(format!("{}/api/query", self.base))
            .query(&[
                ("search_query", search_query.as_str()),
                ("start", "0"),
                ("max_results", max_results.as_str()),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(super::http_error(response.status()));
        }
        let body = response.text().await?;
        Ok((parse_atom(&body)?, Vec::new()))
    }
}

/// Entries of an arXiv Atom feed as results. An empty feed is `Ok([])`;
/// a body that is not Atom at all is a `Parse` error.
pub fn parse_atom(body: &str) -> Result<Vec<RawResult>, ProviderError> {
    let body = body.trim();
    let lower = body.to_ascii_lowercase();
    if !["<feed", "<entry", ":feed", ":entry"]
        .iter()
        .any(|t| lower.contains(t))
    {
        return Err(ProviderError::Parse("not an Atom feed".to_owned()));
    }
    Ok(entries(body, &lower).filter_map(parse_entry).collect())
}

/// Raw `<entry>…</entry>` bodies (exclusive of the tags).
fn entries<'a>(body: &'a str, lower: &'a str) -> impl Iterator<Item = &'a str> {
    let mut from = 0;
    std::iter::from_fn(move || {
        let open = from + find_open_tag(&lower[from..], "entry")?;
        let content_start = open + body[open..].find('>')? + 1;
        let content_end = content_start + find_close_tag(&lower[content_start..], "entry")?;
        from = content_end + lower[content_end..].find('>').map_or(0, |i| i + 1);
        Some(&body[content_start..content_end])
    })
}

fn parse_entry(entry: &str) -> Option<RawResult> {
    let title = text_of(&element_text(entry, "title")?);
    if title.is_empty() {
        return None;
    }
    let url = alternate_link_href(entry)
        .or_else(|| element_text(entry, "id"))
        .map(|u| abs_url(&u))
        .filter(|u| !u.is_empty())?;
    let snippet = text_of(&element_text(entry, "summary").unwrap_or_default());
    let published = element_text(entry, "published")
        .or_else(|| element_text(entry, "updated"))
        .map(|s| collapse_ws(&s))
        .filter(|s| !s.is_empty());
    Some(RawResult {
        title,
        url,
        snippet,
        published,
        score_hint: None,
    })
}

fn text_of(raw: &str) -> String {
    collapse_ws(&decode_entities(raw))
}

/// `http://arxiv.org/…` → `https://arxiv.org/…`; anything else verbatim.
fn abs_url(raw: &str) -> String {
    let t = raw.trim();
    match t
        .strip_prefix("http://arxiv.org/")
        .or_else(|| t.strip_prefix("http://www.arxiv.org/"))
    {
        Some(rest) => format!("https://arxiv.org/{rest}"),
        None => t.to_owned(),
    }
}

/// `href` of the first `<link … rel="alternate" …>`; attribute order varies.
fn alternate_link_href(entry: &str) -> Option<String> {
    let lower = entry.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find("<link") {
        let at = from + rel;
        let gt = entry[at..].find('>')?;
        let tag = &entry[at..=at + gt];
        let tag_lower = &lower[at..=at + gt];
        if (tag_lower.contains("rel=\"alternate\"") || tag_lower.contains("rel='alternate'"))
            && let Some(href) = attr_value(tag, "href")
            && !href.is_empty()
        {
            return Some(href.to_owned());
        }
        from = at + gt + 1;
    }
    None
}

fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=");
    let after = &tag[tag.to_ascii_lowercase().find(&key)? + key.len()..];
    let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = &after[1..];
    Some(&value[..value.find(quote)?])
}

/// Raw text of the first element with local name `local` (prefix allowed).
fn element_text(haystack: &str, local: &str) -> Option<String> {
    let lower = haystack.to_ascii_lowercase();
    let open = find_open_tag(&lower, local)?;
    let content_start = open + haystack[open..].find('>')? + 1;
    if haystack[open..content_start].trim_end().ends_with("/>") {
        return Some(String::new());
    }
    let close = find_close_tag(&lower[content_start..], local)?;
    Some(haystack[content_start..content_start + close].to_owned())
}

/// Byte offset of `<local` or `<prefix:local` at a tag boundary.
fn find_open_tag(lower: &str, local: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = lower[from..].find('<') {
        let at = from + rel;
        from = at + 1;
        let rest = &lower[from..];
        if rest.starts_with('/') {
            continue;
        }
        if local_name(rest) == local {
            return Some(at);
        }
    }
    None
}

/// Byte offset of `</local>` or `</prefix:local>`.
fn find_close_tag(lower: &str, local: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = lower[from..].find("</") {
        let at = from + rel;
        from = at + 2;
        if local_name(&lower[from..]) == local {
            return Some(at);
        }
    }
    None
}

/// Tag name at the start of `rest`, minus any namespace prefix.
fn local_name(rest: &str) -> &str {
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '>' | '/'))
        .unwrap_or(rest.len());
    let name = &rest[..end];
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_tags_and_missing_link_fall_back_to_id() {
        let feed = r#"<atom:feed xmlns:atom="http://www.w3.org/2005/Atom">
          <atom:entry><atom:id>http://arxiv.org/abs/1.2v1</atom:id>
            <atom:title>A &amp; B</atom:title><atom:updated>2020-01-01T00:00:00Z</atom:updated>
            <atom:summary/></atom:entry></atom:feed>"#;
        let hits = parse_atom(feed).expect("atom");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "A & B");
        assert_eq!(hits[0].url, "https://arxiv.org/abs/1.2v1");
        assert_eq!(hits[0].snippet, "");
        assert_eq!(hits[0].published.as_deref(), Some("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn non_atom_body_is_a_parse_error() {
        assert!(matches!(
            parse_atom("<html>maintenance</html>"),
            Err(ProviderError::Parse(_))
        ));
        assert!(parse_atom("<feed></feed>").expect("empty feed").is_empty());
    }
}
