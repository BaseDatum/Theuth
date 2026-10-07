//! Page metadata (design doc 122 D5): a string scan of the first 256 KiB of
//! the HTML (no DOM — `<head>` is what matters) for `<title>`, `<meta>`,
//! `<link rel=canonical>`, `<html lang>` and one JSON-LD article block.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How much of the HTML is scanned.
const SCAN_BYTES: usize = 256 * 1024;
/// JSON-LD blocks over this are skipped.
const MAX_JSON_LD: usize = 64 * 1024;
/// `@type`s whose `author`/`datePublished` are read.
const ARTICLE_TYPES: &[&str] = &[
    "Article",
    "NewsArticle",
    "BlogPosting",
    "TechArticle",
    "ScholarlyArticle",
];

/// Metadata of a fetched page; every field `None` when the page had none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageMeta {
    /// `og:title` → `<title>`.
    pub title: Option<String>,
    /// `meta[name=description]` → `og:description`.
    pub description: Option<String>,
    /// `meta[name=author]` → `article:author` → JSON-LD `author.name`.
    pub author: Option<String>,
    /// RFC 3339 UTC, normalised by `crate::date::normalize`.
    pub published: Option<String>,
    /// `<html lang>`, trimmed and lowercased.
    pub lang: Option<String>,
    /// `link[rel=canonical]`, absolutised against `base`.
    pub canonical: Option<String>,
    /// `og:site_name`.
    pub site_name: Option<String>,
}

impl PageMeta {
    /// Whether every field is `None`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Scan the first 256 KiB of `html` for the D5 sources; first non-empty
/// source per field wins (table in 122 D5).
#[must_use]
pub fn extract(html: &str, base: &url::Url) -> PageMeta {
    let end = (0..=SCAN_BYTES.min(html.len()))
        .rev()
        .find(|&i| html.is_char_boundary(i))
        .unwrap_or(0);
    let html = &html[..end];
    let lower = html.to_ascii_lowercase();

    let mut metas: Vec<(String, String)> = Vec::new(); // (name|property, content)
    let mut canonical = None;
    let mut lang = None;
    for (tag, attrs) in tags(html, &lower) {
        match tag {
            "meta" => {
                let key = attr(&attrs, "property").or_else(|| attr(&attrs, "name"));
                if let (Some(k), Some(c)) = (key, attr(&attrs, "content")) {
                    metas.push((k.to_ascii_lowercase(), c));
                }
            }
            "link" => {
                if canonical.is_none()
                    && attr(&attrs, "rel").is_some_and(|r| {
                        r.split_whitespace()
                            .any(|t| t.eq_ignore_ascii_case("canonical"))
                    })
                    && let Some(href) = attr(&attrs, "href")
                    && let Ok(abs) = base.join(href.trim())
                {
                    canonical = Some(abs.to_string());
                }
            }
            "html" if lang.is_none() => {
                lang = attr(&attrs, "lang").map(|l| l.trim().to_ascii_lowercase());
            }
            _ => {}
        }
    }
    let meta = |key: &str| {
        metas
            .iter()
            .find(|(k, v)| k == key && !v.trim().is_empty())
            .map(|(_, v)| v.trim().to_owned())
    };
    let ld = json_ld(html, &lower);
    let author = meta("author")
        .or_else(|| meta("article:author"))
        .or_else(|| ld.as_ref().and_then(ld_author));
    let published = meta("article:published_time")
        .or_else(|| {
            ld.as_ref()
                .and_then(|v| v.get("datePublished"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .or_else(|| meta("date"))
        .and_then(|raw| crate::date::normalize(&raw, chrono::Utc::now()));
    PageMeta {
        title: meta("og:title").or_else(|| title(html, &lower)),
        description: meta("description").or_else(|| meta("og:description")),
        author,
        published,
        lang: lang.filter(|l| !l.is_empty()),
        canonical,
        site_name: meta("og:site_name"),
    }
}

/// JSON-LD `author.name` (object or first of an array; a bare string author counts).
fn ld_author(ld: &serde_json::Value) -> Option<String> {
    let a = ld.get("author")?;
    let a = a.as_array().map_or(a, |arr| arr.first().unwrap_or(a));
    let name = a.as_str().or_else(|| a.get("name")?.as_str())?;
    (!name.trim().is_empty()).then(|| name.trim().to_owned())
}

/// The first `<script type="application/ld+json">` block whose `@type` is an
/// article type, parsed; blocks over 64 KiB and parse errors are skipped.
fn json_ld(html: &str, lower: &str) -> Option<serde_json::Value> {
    let mut from = 0;
    while let Some(i) = lower[from..].find("<script") {
        let open = from + i;
        let gt = open + lower[open..].find('>')?;
        let attrs = attrs(&html[open + "<script".len()..gt]);
        let close = gt + 1 + lower[gt + 1..].find("</script")?;
        from = close;
        if !attr(&attrs, "type")
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/ld+json"))
        {
            continue;
        }
        let body = &html[gt + 1..close];
        if body.len() > MAX_JSON_LD {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
            continue;
        };
        let ty = v.get("@type")?;
        let ty = ty.as_array().map_or(ty, |arr| arr.first().unwrap_or(ty));
        if ty.as_str().is_some_and(|t| ARTICLE_TYPES.contains(&t)) {
            return Some(v);
        }
    }
    None
}

/// `<title>` text, entities decoded, whitespace collapsed.
fn title(html: &str, lower: &str) -> Option<String> {
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let t = super::collapse_ws(&unescape(&html[start..end]));
    (!t.is_empty()).then_some(t)
}

/// Every `<meta>`, `<link>` and `<html>` start tag with its attributes.
fn tags<'a>(
    html: &'a str,
    lower: &'a str,
) -> impl Iterator<Item = (&'static str, Vec<(String, String)>)> + 'a {
    let mut from = 0;
    std::iter::from_fn(move || {
        loop {
            let i = lower[from..].find('<')?;
            let open = from + i;
            let rest = &lower[open + 1..];
            let gt = rest.find('>')?;
            from = open + 1 + gt + 1;
            let name_end = rest
                .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                .unwrap_or(gt);
            let tag = match &rest[..name_end.min(gt)] {
                "meta" => "meta",
                "link" => "link",
                "html" => "html",
                // `<body>`: the head is over; nothing below carries metadata.
                "body" => return None,
                _ => continue,
            };
            let attr_text = &html[open + 1 + name_end..open + 1 + gt];
            return Some((tag, attrs(attr_text)));
        }
    })
}

/// `name="value"`, `name='value'`, `name=value` and bare `name` pairs; names lowercased.
fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some(&(start, c)) = it.peek() {
        if c.is_ascii_whitespace() || c == '/' {
            it.next();
            continue;
        }
        let mut end = start;
        while let Some(&(i, c)) = it.peek() {
            if c.is_ascii_whitespace() || c == '=' || c == '/' {
                break;
            }
            end = i + c.len_utf8();
            it.next();
        }
        let name = s[start..end].to_ascii_lowercase();
        while it.peek().is_some_and(|(_, c)| c.is_ascii_whitespace()) {
            it.next();
        }
        let mut value = String::new();
        if it.peek().is_some_and(|(_, c)| *c == '=') {
            it.next();
            while it.peek().is_some_and(|(_, c)| c.is_ascii_whitespace()) {
                it.next();
            }
            match it.peek().map(|(_, c)| *c) {
                Some(q @ ('"' | '\'')) => {
                    it.next();
                    for (_, c) in it.by_ref() {
                        if c == q {
                            break;
                        }
                        value.push(c);
                    }
                }
                Some(_) => {
                    while let Some(&(_, c)) = it.peek() {
                        if c.is_ascii_whitespace() {
                            break;
                        }
                        value.push(c);
                        it.next();
                    }
                }
                None => {}
            }
        }
        if !name.is_empty() {
            out.push((name, unescape(&value)));
        }
    }
    out
}

fn attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
        .filter(|v| !v.trim().is_empty())
}

/// The handful of entities that reach `<head>` text.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> url::Url {
        url::Url::parse("https://example.test/blog/post?x=1").unwrap()
    }

    #[test]
    fn each_source_row() {
        let html = r#"<!doctype html><html lang=" EN-gb "><head>
<meta charset="utf-8">
<title>Plain &amp; Title</title>
<meta property="og:title" content="OG Title">
<meta name="description" content="Desc here">
<meta property="og:description" content="OG desc">
<meta name="author" content="Jane Doe">
<meta property="article:published_time" content="2026-09-30T08:00:00+02:00">
<link rel="stylesheet" href="/a.css"><link rel="canonical" href="../canonical/">
<meta property="og:site_name" content="Example">
</head><body><p>x</p></body></html>"#;
        let m = extract(html, &base());
        assert_eq!(
            m,
            PageMeta {
                title: Some("OG Title".into()),
                description: Some("Desc here".into()),
                author: Some("Jane Doe".into()),
                published: Some("2026-09-30T06:00:00Z".into()),
                lang: Some("en-gb".into()),
                canonical: Some("https://example.test/canonical/".into()),
                site_name: Some("Example".into()),
            }
        );
    }

    #[test]
    fn fallbacks_and_json_ld() {
        let html = r#"<html><head><title> Fallback  Title </title>
<meta property="og:description" content="only og">
<script type="application/ld+json">{"@type":"WebPage","author":{"name":"Nope"}}</script>
<script type="application/ld+json">{"@context":"https://schema.org","@type":["NewsArticle"],
 "author":[{"@type":"Person","name":"Ada Lovelace"}],"datePublished":"September 30, 2026"}</script>
</head><body><p>body</p>"#;
        let m = extract(html, &base());
        assert_eq!(m.title.as_deref(), Some("Fallback Title"));
        assert_eq!(m.description.as_deref(), Some("only og"));
        assert_eq!(m.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(m.published.as_deref(), Some("2026-09-30T00:00:00Z"));
        assert_eq!(m.lang, None);
        assert_eq!(m.canonical, None);
    }

    #[test]
    fn unparsable_date_and_empty_page() {
        let html = r#"<head><meta name="date" content="not a date"><meta name="author" content="  "></head>"#;
        let m = extract(html, &base());
        assert_eq!(m.published, None);
        assert_eq!(m.author, None);
        assert!(extract("<p>no head</p>", &base()).is_empty());
    }

    #[test]
    fn attr_parser_handles_quote_styles() {
        let a = attrs(r#" name=viewport content='a "b"' data-x async href="/p?a=1&amp;b=2" "#);
        assert_eq!(attr(&a, "name").as_deref(), Some("viewport"));
        assert_eq!(attr(&a, "content").as_deref(), Some(r#"a "b""#));
        assert_eq!(attr(&a, "href").as_deref(), Some("/p?a=1&b=2"));
        assert!(a.iter().any(|(k, v)| k == "async" && v.is_empty()));
    }
}
