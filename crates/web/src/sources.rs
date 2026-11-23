//! Known JSON sources (design doc 122 D11): hosts agents read most, read from
//! their public JSON endpoint instead of the HTML. Eight rows: [`api_source`]
//! matches a page URL on exact host and path shape and names the endpoint;
//! [`render`] turns the fetched body into compact markdown. Pure: no network.

use std::fmt::Write as _;

use html_to_markdown_rs::{ConversionOptions, convert};
use serde_json::Value;
use url::Url;

use crate::engines::arxiv::parse_atom;
use crate::engines::decode_entities;
use crate::fetch::collapse_ws;

/// A page URL that has a JSON endpoint worth reading instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiSource {
    /// Table key: `hn-item`, `reddit-old`, `npm`, `pypi`, `crates-io`, `arxiv`, `wikipedia`, `stackoverflow`.
    pub kind: &'static str,
    /// The URL to fetch instead of the page; always re-checked by the SSRF guard.
    pub fetch_url: Url,
}

/// README / long-description head kept for `npm` and `pypi`.
const DESCRIPTION_CHARS: usize = 3_000;
/// Wikipedia extract head kept.
const WIKIPEDIA_CHARS: usize = 12_000;
/// HN comment tree limits.
const HN_MAX_COMMENTS: usize = 60;
const HN_MAX_DEPTH: usize = 3;
/// Stack Overflow answers shown.
const SO_TOP_ANSWERS: usize = 3;

/// Wikipedia namespaces that are not articles (lowercase, before the colon).
const WIKIPEDIA_NAMESPACES: &[&str] = &[
    "special",
    "file",
    "image",
    "media",
    "category",
    "talk",
    "wikipedia",
    "project",
    "help",
    "template",
    "portal",
    "user",
    "draft",
    "module",
    "mediawiki",
    "book",
    "timedtext",
    "topic",
];

/// The D11 table, matched on host and path; `None` for everything else.
#[must_use]
pub fn api_source(url: &Url) -> Option<ApiSource> {
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let segments: Vec<String> = url
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect();
    let segs: Vec<&str> = segments.iter().map(String::as_str).collect();
    let (kind, fetch_url) = match host {
        "news.ycombinator.com" => hn_item(url, &segs)?,
        "reddit.com" => reddit_old(url, &segs)?,
        h if h.ends_with(".reddit.com") => reddit_old(url, &segs)?,
        "npmjs.com" => npm(&segs)?,
        "pypi.org" => pypi(&segs)?,
        "crates.io" => crates_io(&segs)?,
        "arxiv.org" => arxiv(&segs)?,
        "stackoverflow.com" => stackoverflow(&segs)?,
        h if wikipedia_lang(h).is_some() => wikipedia(url, &segs)?,
        _ => return None,
    };
    Some(ApiSource { kind, fetch_url })
}

/// A second endpoint whose body must be appended (newline-separated) to the
/// first before [`render`]: Stack Overflow's answers. `None` for every other kind.
#[must_use]
pub fn follow_up(source: &ApiSource) -> Option<Url> {
    if source.kind != "stackoverflow" {
        return None;
    }
    let mut url = source.fetch_url.clone();
    url.path_segments_mut().ok()?.push("answers");
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("site", "stackoverflow")
        .append_pair("filter", "withbody")
        .append_pair("sort", "votes");
    Some(url)
}

/// `base` with `segments` appended as percent-encoded path segments.
fn at(base: &str, segments: &[&str]) -> Option<Url> {
    let mut url = Url::parse(base).ok()?;
    url.path_segments_mut().ok()?.extend(segments);
    Some(url)
}

fn hn_item(url: &Url, segs: &[&str]) -> Option<(&'static str, Url)> {
    if segs != ["item"] {
        return None;
    }
    let id = url.query_pairs().find(|(k, _)| k == "id")?.1.into_owned();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        "hn-item",
        at("https://hn.algolia.com/api/v1/items", &[&id])?,
    ))
}

/// `/r/<sub>/comments/<id>…` only; the path is kept, the host becomes `old.reddit.com`.
fn reddit_old(url: &Url, segs: &[&str]) -> Option<(&'static str, Url)> {
    match segs {
        ["r", sub, "comments", id, ..] if !sub.is_empty() && !id.is_empty() => {
            let mut out = Url::parse("https://old.reddit.com/").ok()?;
            out.set_path(url.path());
            Some(("reddit-old", out))
        }
        _ => None,
    }
}

fn npm(segs: &[&str]) -> Option<(&'static str, Url)> {
    let path: &[&str] = match segs {
        ["package", scope, name, ..] if scope.starts_with('@') && !name.is_empty() => {
            &[scope, name]
        }
        ["package", name, ..] if !name.is_empty() && !name.starts_with('@') => &[name],
        _ => return None,
    };
    Some(("npm", at("https://registry.npmjs.org", path)?))
}

fn pypi(segs: &[&str]) -> Option<(&'static str, Url)> {
    match segs {
        ["project", name, ..] if !name.is_empty() => {
            Some(("pypi", at("https://pypi.org/pypi", &[name, "json"])?))
        }
        _ => None,
    }
}

fn crates_io(segs: &[&str]) -> Option<(&'static str, Url)> {
    match segs {
        ["crates", name, ..] if !name.is_empty() => {
            Some(("crates-io", at("https://crates.io/api/v1/crates", &[name])?))
        }
        _ => None,
    }
}

/// `/abs/<id>`; old-style ids (`hep-th/9901001`) span two segments.
fn arxiv(segs: &[&str]) -> Option<(&'static str, Url)> {
    let ["abs", id @ ..] = segs else {
        return None;
    };
    if id.is_empty() || id.len() > 2 {
        return None;
    }
    let mut url = Url::parse("https://export.arxiv.org/api/query").ok()?;
    url.query_pairs_mut().append_pair("id_list", &id.join("/"));
    Some(("arxiv", url))
}

fn stackoverflow(segs: &[&str]) -> Option<(&'static str, Url)> {
    match segs {
        ["questions" | "q", id, ..] if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => {
            let mut url = at("https://api.stackexchange.com/2.3/questions", &[id])?;
            url.query_pairs_mut()
                .append_pair("site", "stackoverflow")
                .append_pair("filter", "withbody");
            Some(("stackoverflow", url))
        }
        _ => None,
    }
}

/// The language label of `<lang>.wikipedia.org`; `None` for any other shape.
fn wikipedia_lang(host: &str) -> Option<&str> {
    let (lang, rest) = host.split_once('.')?;
    (rest == "wikipedia.org"
        && !lang.is_empty()
        && lang != "m"
        && lang.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
    .then_some(lang)
}

/// `/wiki/<title>` for an article title (no namespace prefix).
fn wikipedia(url: &Url, segs: &[&str]) -> Option<(&'static str, Url)> {
    let ["wiki", title] = segs else {
        return None;
    };
    if title.is_empty() {
        return None;
    }
    if let Some((ns, _)) = title.split_once(':') {
        let ns = ns.to_ascii_lowercase().replace('_', " ");
        if WIKIPEDIA_NAMESPACES.contains(&ns.as_str()) || ns.ends_with(" talk") {
            return None;
        }
    }
    let mut out = url.clone();
    out.set_scheme("https").ok()?;
    out.set_port(None).ok()?;
    out.set_path("/w/api.php");
    out.set_fragment(None);
    out.set_query(None);
    out.query_pairs_mut()
        .append_pair("action", "query")
        .append_pair("prop", "extracts")
        .append_pair("explaintext", "1")
        .append_pair("format", "json")
        .append_pair("titles", title);
    Some(("wikipedia", out))
}

/// `%XX` sequences decoded; invalid UTF-8 is replaced.
fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

// ---- rendering ---------------------------------------------------------------

/// Render a fetched body of `kind` as compact markdown; `None` when the body
/// does not parse. `reddit-old` has no renderer: its body is HTML for the
/// normal page pipeline.
#[must_use]
pub fn render(kind: &str, body: &str) -> Option<String> {
    let out = match kind {
        "hn-item" => hn_markdown(&json(body)?),
        "npm" => npm_markdown(&json(body)?),
        "pypi" => pypi_markdown(&json(body)?),
        "crates-io" => crates_markdown(&json(body)?),
        "arxiv" => arxiv_markdown(body),
        "wikipedia" => wikipedia_markdown(&json(body)?),
        "stackoverflow" => stackoverflow_markdown(body),
        _ => None,
    }?;
    (!out.trim().is_empty()).then_some(out)
}

fn json(body: &str) -> Option<Value> {
    serde_json::from_str(body).ok()
}

/// Non-empty trimmed string at `key`.
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A string, or the `key` member of an object (`license: {type}`, `repository: {url}`).
fn field_or_member<'a>(v: &'a Value, key: &str, member: &str) -> Option<&'a str> {
    field(v, key).or_else(|| field(v.get(key)?, member))
}

/// HTML fragment → markdown (HN texts, Stack Exchange bodies).
fn html_md(html: &str) -> String {
    let opts = ConversionOptions {
        extract_metadata: false,
        ..ConversionOptions::default()
    };
    convert(html, opts)
        .ok()
        .and_then(|r| r.content)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// First `n` chars of `s`, with an ellipsis when cut.
fn head(s: &str, n: usize) -> String {
    let mut out: String = s.trim().chars().take(n).collect();
    if out.len() < s.trim().len() {
        out.push('…');
    }
    out
}

/// `a · b · c` of the present parts.
fn meta_line(parts: &[Option<String>]) -> Option<String> {
    let present: Vec<&str> = parts.iter().flatten().map(String::as_str).collect();
    (!present.is_empty()).then(|| present.join(" · "))
}

fn labelled(label: &str, value: Option<&str>) -> Option<String> {
    value.map(|v| format!("{label} {v}"))
}

fn paragraphs(parts: Vec<Option<String>>) -> String {
    parts
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn hn_markdown(v: &Value) -> Option<String> {
    let title = field(v, "title")?;
    let header = meta_line(&[
        labelled("by", field(v, "author")),
        v.get("points")
            .and_then(Value::as_u64)
            .map(|p| format!("{p} points")),
        field(v, "created_at").map(str::to_owned),
    ]);
    let mut comments = String::new();
    let mut budget = HN_MAX_COMMENTS;
    hn_comments(v, 1, &mut budget, &mut comments);
    Some(paragraphs(vec![
        Some(format!("# {title}")),
        header,
        field(v, "url").map(str::to_owned),
        field(v, "text").map(html_md),
        (!comments.is_empty()).then(|| comments.trim_end().to_owned()),
    ]))
}

/// Nested `- **author**: text` lines, depth ≤ 3, at most `budget` comments.
fn hn_comments(v: &Value, depth: usize, budget: &mut usize, out: &mut String) {
    if depth > HN_MAX_DEPTH {
        return;
    }
    for child in v
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if *budget == 0 {
            return;
        }
        let Some(text) = field(child, "text") else {
            continue; // deleted or dead
        };
        let author = field(child, "author").unwrap_or("[deleted]");
        let text = collapse_ws(&html_md(text));
        *budget -= 1;
        let _ = writeln!(out, "{}- **{author}**: {text}", "  ".repeat(depth - 1));
        hn_comments(child, depth + 1, budget, out);
    }
}

fn npm_markdown(v: &Value) -> Option<String> {
    let name = field(v, "name")?;
    let latest = v.get("dist-tags").and_then(|t| field(t, "latest"));
    let version = latest
        .and_then(|l| v.get("versions")?.get(l))
        .unwrap_or(&Value::Null);
    let pick = |key: &str, member: &str| {
        field_or_member(version, key, member)
            .or_else(|| field_or_member(v, key, member))
            .map(str::to_owned)
    };
    let repository = pick("repository", "url").map(|r| {
        r.trim_start_matches("git+")
            .trim_end_matches(".git")
            .to_owned()
    });
    Some(paragraphs(vec![
        Some(
            format!("# {name} {}", latest.unwrap_or(""))
                .trim_end()
                .to_owned(),
        ),
        pick("description", ""),
        meta_line(&[
            labelled("license", pick("license", "type").as_deref()),
            labelled("homepage", pick("homepage", "").as_deref()),
            labelled("repository", repository.as_deref()),
        ]),
        field(v, "readme").map(|r| head(r, DESCRIPTION_CHARS)),
    ]))
}

fn pypi_markdown(v: &Value) -> Option<String> {
    let info = v.get("info")?;
    let name = field(info, "name")?;
    let version = field(info, "version")?;
    let license = field(info, "license_expression")
        .or_else(|| field(info, "license").filter(|l| l.len() <= 80));
    let home = field(info, "home_page")
        .or_else(|| field(info.get("project_urls")?, "Homepage"))
        .or_else(|| field(info.get("project_urls")?, "Source"));
    Some(paragraphs(vec![
        Some(format!("# {name} {version}")),
        field(info, "summary").map(str::to_owned),
        meta_line(&[
            labelled("requires_python", field(info, "requires_python")),
            labelled("license", license),
            labelled("home_page", home),
        ]),
        field(info, "description").map(|d| head(d, DESCRIPTION_CHARS)),
    ]))
}

fn crates_markdown(v: &Value) -> Option<String> {
    let c = v.get("crate")?;
    let name = field(c, "name")?;
    let version = field(c, "max_stable_version").or_else(|| field(c, "max_version"))?;
    let keywords: Vec<&str> = c
        .get("keywords")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    Some(paragraphs(vec![
        Some(format!("# {name} {version}")),
        field(c, "description").map(str::to_owned),
        meta_line(&[
            c.get("downloads")
                .and_then(Value::as_u64)
                .map(|d| format!("downloads {d}")),
            labelled("repository", field(c, "repository")),
            labelled("documentation", field(c, "documentation")),
        ]),
        (!keywords.is_empty()).then(|| format!("keywords: {}", keywords.join(", "))),
    ]))
}

fn arxiv_markdown(body: &str) -> Option<String> {
    let entry = parse_atom(body).ok()?.into_iter().next()?;
    let authors: Vec<String> = xml_elements(body, "name")
        .map(|n| collapse_ws(&decode_entities(n)))
        .filter(|n| !n.is_empty())
        .collect();
    let pdf = xml_attr_of_tag(body, "<link", "title=\"pdf\"", "href")
        .map(str::to_owned)
        .or_else(|| {
            entry
                .url
                .contains("/abs/")
                .then(|| entry.url.replacen("/abs/", "/pdf/", 1))
        });
    Some(paragraphs(vec![
        Some(format!("# {}", entry.title)),
        (!authors.is_empty()).then(|| authors.join(", ")),
        labelled("published", entry.published.as_deref()),
        (!entry.snippet.is_empty()).then(|| entry.snippet.clone()),
        labelled("PDF:", pdf.as_deref()),
    ]))
}

/// Raw inner text of every `<local>…</local>` element (no attributes, no prefix).
fn xml_elements<'a>(body: &'a str, local: &str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{local}>");
    let close = format!("</{local}>");
    let mut from = 0;
    std::iter::from_fn(move || {
        let start = from + body[from..].find(&open)? + open.len();
        let end = start + body[start..].find(&close)?;
        from = end + close.len();
        Some(&body[start..end])
    })
}

/// `attr` of the first `tag …>` that contains `marker`.
fn xml_attr_of_tag<'a>(body: &'a str, tag: &str, marker: &str, attr: &str) -> Option<&'a str> {
    let key = format!("{attr}=\"");
    let mut from = 0;
    while let Some(rel) = body[from..].find(tag) {
        let at = from + rel;
        let end = at + body[at..].find('>')?;
        let element = &body[at..end];
        if element.contains(marker)
            && let Some(start) = element.find(&key)
        {
            let value = &element[start + key.len()..];
            return value.split('"').next();
        }
        from = end + 1;
    }
    None
}

fn wikipedia_markdown(v: &Value) -> Option<String> {
    let pages = v.get("query")?.get("pages")?;
    let page = match pages {
        Value::Object(map) => map.values().next()?,
        Value::Array(items) => items.first()?,
        _ => return None,
    };
    if page.get("missing").is_some() || page.get("invalid").is_some() {
        return None;
    }
    let title = field(page, "title")?;
    let extract = field(page, "extract")?;
    Some(format!("# {title}\n\n{}", head(extract, WIKIPEDIA_CHARS)))
}

/// `body` is one or more Stack Exchange responses (the question, then the
/// answers), concatenated or wrapped in one JSON array.
fn stackoverflow_markdown(body: &str) -> Option<String> {
    let mut items = Vec::new();
    for doc in serde_json::Deserializer::from_str(body).into_iter::<Value>() {
        so_items(doc.ok()?, &mut items);
    }
    let question = items
        .iter()
        .find(|i| i.get("question_id").is_some() && i.get("title").is_some())?
        .clone();
    let mut answers: Vec<Value> = items
        .into_iter()
        .filter(|i| i.get("answer_id").is_some())
        .chain(
            question
                .get("answers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        )
        .collect();
    answers.sort_by_key(|a| {
        (
            !a.get("is_accepted")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            -a.get("score").and_then(Value::as_i64).unwrap_or(0),
        )
    });
    answers.dedup_by_key(|a| a.get("answer_id").cloned());
    let title = decode_entities(field(&question, "title")?);
    let tags: Vec<&str> = question
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let owner = |v: &Value| {
        v.get("owner")
            .and_then(|o| field(o, "display_name"))
            .map(decode_entities)
    };
    let mut parts = vec![
        Some(format!("# {title}")),
        meta_line(&[
            question
                .get("score")
                .and_then(Value::as_i64)
                .map(|s| format!("score {s}")),
            labelled("asked by", owner(&question).as_deref()),
            (!tags.is_empty()).then(|| format!("tags: {}", tags.join(", "))),
        ]),
        field(&question, "body").map(html_md),
    ];
    if !answers.is_empty() {
        parts.push(Some("## Answers".to_owned()));
        for a in answers.iter().take(SO_TOP_ANSWERS) {
            let accepted = a
                .get("is_accepted")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            parts.push(meta_line(&[
                accepted
                    .then(|| "### ✓ accepted".to_owned())
                    .or_else(|| Some("###".to_owned())),
                a.get("score")
                    .and_then(Value::as_i64)
                    .map(|s| format!("score {s}")),
                labelled("by", owner(a).as_deref()),
            ]));
            parts.push(field(a, "body").map(html_md));
        }
    }
    Some(paragraphs(parts))
}

/// Flatten a response (or an array of responses) into its `items`.
fn so_items(doc: Value, out: &mut Vec<Value>) {
    match doc {
        Value::Array(docs) => docs.into_iter().for_each(|d| so_items(d, out)),
        Value::Object(mut map) => {
            if let Some(Value::Array(items)) = map.remove("items") {
                out.extend(items);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(
            percent_decode("Rust_(programming_language)"),
            "Rust_(programming_language)"
        );
        assert_eq!(percent_decode("caf%C3%A9%20au%2Flait"), "café au/lait");
        assert_eq!(percent_decode("bad%zz%4"), "bad%zz%4");
    }

    #[test]
    fn wikipedia_language_labels() {
        assert_eq!(wikipedia_lang("en.wikipedia.org"), Some("en"));
        assert_eq!(wikipedia_lang("zh-yue.wikipedia.org"), Some("zh-yue"));
        assert_eq!(wikipedia_lang("en.m.wikipedia.org"), None);
        assert_eq!(wikipedia_lang("wikipedia.org"), None);
        assert_eq!(wikipedia_lang("en.wikipedia.org.evil.test"), None);
    }

    #[test]
    fn head_marks_a_cut() {
        assert_eq!(head("abcdef", 3), "abc…");
        assert_eq!(head(" abc ", 3), "abc");
    }
}
