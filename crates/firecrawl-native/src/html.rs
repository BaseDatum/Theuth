use std::collections::{HashMap, HashSet};

use std::sync::LazyLock;

use kuchikiki::{NodeRef, iter::NodeEdge, parse_html, traits::TendrilSink};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"url\(['"]?([^'")]+)['"]?\)"#).expect("URL_REGEX is a valid static regex pattern")
});

const LENGTH_THRESHOLD: usize = 6;
const SIGNATURE_VERSION: u8 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
struct SignatureMode {
    class: bool,
    id: bool,
    text: bool,
}

impl From<String> for SignatureMode {
    fn from(s: String) -> Self {
        let mut mode = SignatureMode {
            class: false,
            id: false,
            text: false,
        };
        for c in s.chars() {
            match c {
                'c' => mode.class = true,
                'i' => mode.id = true,
                't' => mode.text = true,
                _ => {}
            }
        }
        mode
    }
}

impl From<SignatureMode> for String {
    fn from(mode: SignatureMode) -> Self {
        let mut s = String::new();
        if mode.class {
            s.push('c');
        }
        if mode.id {
            s.push('i');
        }
        if mode.text {
            s.push('t');
        }
        s
    }
}

fn get_node_self_signature(node: &kuchikiki::NodeRef, mode: SignatureMode) -> String {
    let mut signature = format!(
        "v{}:{}:",
        SIGNATURE_VERSION,
        <SignatureMode as Into<String>>::into(mode)
    );
    if node.text_contents().trim().is_empty() {
        return String::new();
    }

    if let Some(element) = node.as_element() {
        signature.push_str(&element.name.local);
        let attributes = element.attributes.borrow();
        if mode.id {
            if let Some(id) = attributes.get("id") {
                signature.push_str(id);
            }
        }
        if mode.class {
            if let Some(class) = attributes.get("class") {
                signature.push_str(class);
            }
        }
    }

    if mode.text {
        if let Some(text) = node.as_text() {
            signature.push_str(&text.borrow().to_string());
        }
    }
    signature
}

fn get_node_signature(node: &kuchikiki::NodeRef, mode: SignatureMode) -> String {
    if node.text_contents().trim().len() < LENGTH_THRESHOLD {
        return String::new();
    }

    let mut signature = get_node_self_signature(node, mode);

    let mut child = node.first_child();
    while let Some(ref c) = child {
        let child_signature = get_node_signature(c, mode);
        signature.push_str(&child_signature);
        child = c.next_sibling();
    }

    if signature.is_empty() {
        return signature;
    }

    format!(
        "v{}:{}:{}",
        SIGNATURE_VERSION,
        <SignatureMode as Into<String>>::into(mode),
        platform::sha256::hex(signature.as_bytes())
    )
}

pub fn extract_base_href_from_document(document: &NodeRef, url: &Url) -> crate::Result<String> {
    if let Some(base) = document
        .select("base[href]")
        .map_err(|_| "Failed to select base href".to_string())?
        .next()
        .and_then(|base| base.attributes.borrow().get("href").map(|x| x.to_string()))
    {
        if let Ok(base) = url.join(&base) {
            return Ok(base.to_string());
        }
    }

    Ok(url.to_string())
}

pub fn extract_base_href(html: &str, url: &str) -> crate::Result<String> {
    let document = parse_html().one(html);
    let url = Url::parse(url)?;
    extract_base_href_from_document(&document, &url)
}

/// Extract all links from HTML document.
pub fn extract_links(html: Option<String>) -> crate::Result<Vec<String>> {
    let html = match html {
        Some(h) => h,
        None => return Ok(Vec::new()),
    };

    let document = parse_html().one(html.as_str());

    let anchors: Vec<_> = document
        .select("a[href]")
        .map_err(|_| crate::NativeError::new("Failed to select links"))?
        .collect();

    let mut out: Vec<String> = Vec::new();

    for anchor in anchors {
        let mut href = match anchor.attributes.borrow().get("href") {
            Some(x) => x.to_string(),
            None => continue,
        };

        if href.starts_with("http:/") && !href.starts_with("http://") {
            href = format!("http://{}", &href[6..]);
        } else if href.starts_with("https:/") && !href.starts_with("https://") {
            href = format!("https://{}", &href[7..]);
        }

        out.push(href);
    }

    Ok(out)
}

macro_rules! insert_meta_name {
    ($out:ident, $document:ident, $metaName:expr, $outName:expr) => {
        if let Some(x) = $document
            .select(&format!("meta[name=\"{}\"]", $metaName))
            .map_err(|_| "Failed to select meta name")?
            .next()
            .and_then(|description| {
                description
                    .attributes
                    .borrow()
                    .get("content")
                    .map(|x| x.to_string())
            })
        {
            $out.insert(($outName).to_string(), Value::String(x));
        }
    };
}

macro_rules! insert_meta_property {
    ($out:ident, $document:ident, $metaName:expr, $outName:expr) => {
        if let Some(x) = $document
            .select(&format!("meta[property=\"{}\"]", $metaName))
            .map_err(|_| "Failed to select meta property")?
            .next()
            .and_then(|description| {
                description
                    .attributes
                    .borrow()
                    .get("content")
                    .map(|x| x.to_string())
            })
        {
            $out.insert(($outName).to_string(), Value::String(x));
        }
    };
}

pub fn extract_metadata(html: &str) -> crate::Result<HashMap<String, Value>> {
    let document = parse_html().one(html);
    let mut out = HashMap::<String, Value>::new();

    let head_node = document
        .select("head")
        .map_err(|_| "Failed to select head")?
        .next();

    let search_root = head_node.as_ref().map(|h| h.as_node()).unwrap_or(&document);

    if let Some(title) = search_root
        .select("title")
        .map_err(|_| "Failed to select title")?
        .next()
    {
        out.insert("title".to_string(), Value::String(title.text_contents()));
    }

    if let Some(favicon_link) = search_root
        .select("link[rel=\"icon\"]")
        .map_err(|_| "Failed to select favicon")?
        .next()
        .and_then(|x| x.attributes.borrow().get("href").map(|x| x.to_string()))
        .or_else(|| {
            search_root
                .select("link[rel*=\"icon\"]")
                .ok()
                .and_then(|mut x| {
                    x.next()
                        .and_then(|x| x.attributes.borrow().get("href").map(|x| x.to_string()))
                })
        })
    {
        out.insert("favicon".to_string(), Value::String(favicon_link));
    }

    if let Some(lang) = document
        .select("html[lang]")
        .map_err(|_| "Failed to select lang")?
        .next()
        .and_then(|x| x.attributes.borrow().get("lang").map(|x| x.to_string()))
    {
        out.insert("language".to_string(), Value::String(lang));
    }

    insert_meta_property!(out, search_root, "og:title", "ogTitle");
    insert_meta_property!(out, search_root, "og:description", "ogDescription");
    insert_meta_property!(out, search_root, "og:url", "ogUrl");
    insert_meta_property!(out, search_root, "og:image", "ogImage");
    insert_meta_property!(out, search_root, "og:audio", "ogAudio");
    insert_meta_property!(out, search_root, "og:determiner", "ogDeterminer");
    insert_meta_property!(out, search_root, "og:locale", "ogLocale");

    for meta in search_root
        .select("meta[property=\"og:locale:alternate\"]")
        .map_err(|_| "Failed to select og locale alternate")?
    {
        let attrs = meta.attributes.borrow();

        if let Some(content) = attrs.get("content") {
            if let Some(v) = out.get_mut("ogLocaleAlternate") {
                match v {
                    Value::Array(x) => x.push(Value::String(content.to_string())),
                    _ => unreachable!(),
                }
            } else {
                out.insert(
                    "ogLocaleAlternate".to_string(),
                    Value::Array(vec![Value::String(content.to_string())]),
                );
            }
        }
    }

    insert_meta_property!(out, document, "og:site_name", "ogSiteName");
    insert_meta_property!(out, document, "og:video", "ogVideo");
    insert_meta_name!(out, document, "article:section", "articleSection");
    insert_meta_name!(out, document, "article:tag", "articleTag");
    insert_meta_property!(out, document, "article:published_time", "publishedTime");
    insert_meta_property!(out, document, "article:modified_time", "modifiedTime");
    insert_meta_name!(out, document, "dcterms.keywords", "dcTermsKeywords");
    insert_meta_name!(out, document, "dc.description", "dcDescription");
    insert_meta_name!(out, document, "dc.subject", "dcSubject");
    insert_meta_name!(out, document, "dcterms.subject", "dcTermsSubject");
    insert_meta_name!(out, document, "dcterms.audience", "dcTermsAudience");
    insert_meta_name!(out, document, "dc.type", "dcType");
    insert_meta_name!(out, document, "dcterms.type", "dcTermsType");
    insert_meta_name!(out, document, "dc.date", "dcDate");
    insert_meta_name!(out, document, "dc.date.created", "dcDateCreated");
    insert_meta_name!(out, document, "dcterms.created", "dcTermsCreated");

    for meta in document
        .select("meta")
        .map_err(|_| "Failed to select meta")?
    {
        let meta = meta.as_node().as_element().unwrap();
        let attrs = meta.attributes.borrow();

        if let Some(name) = attrs
            .get("name")
            .or_else(|| attrs.get("property"))
            .or_else(|| attrs.get("itemprop"))
        {
            if let Some(content) = attrs.get("content") {
                if let Some(v) = out.get(name) {
                    match v {
                        Value::String(existing) => {
                            if name == "description" {
                                out.insert(
                                    name.to_string(),
                                    Value::String(format!("{existing}, {content}")),
                                );
                            } else if name != "title" {
                                out.insert(
                                    name.to_string(),
                                    Value::Array(vec![
                                        Value::String(existing.clone()),
                                        Value::String(content.to_string()),
                                    ]),
                                );
                            }
                        }
                        Value::Array(existing_array) => {
                            if name == "description" {
                                let mut values: Vec<String> = existing_array
                                    .iter()
                                    .filter_map(|v| match v {
                                        Value::String(s) => Some(s.clone()),
                                        _ => None,
                                    })
                                    .collect();
                                values.push(content.to_string());
                                out.insert(name.to_string(), Value::String(values.join(", ")));
                            } else {
                                match out.get_mut(name) {
                                    Some(Value::Array(x)) => {
                                        x.push(Value::String(content.to_string()))
                                    }
                                    _ => unreachable!(),
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                } else {
                    out.insert(name.to_string(), Value::String(content.to_string()));
                }
            }
        }
    }

    // Backfill title from og:title, twitter:title, or meta[name="title"] if primary extraction failed
    if !out.contains_key("title") {
        let fallback_title = out
            .get("ogTitle")
            .or_else(|| out.get("og:title"))
            .or_else(|| out.get("twitter:title"))
            .and_then(|v| match v {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            });

        if let Some(title) = fallback_title {
            out.insert("title".to_string(), Value::String(title));
        }
    }

    Ok(out)
}

pub(crate) const EXCLUDE_NON_MAIN_TAGS: [&str; 42] = [
    "header",
    "footer",
    "nav",
    "aside",
    ".header",
    ".top",
    ".navbar",
    "#header",
    ".footer",
    ".bottom",
    "#footer",
    ".sidebar",
    ".side",
    ".aside",
    "#sidebar",
    ".modal",
    ".popup",
    "#modal",
    ".overlay",
    ".ad",
    ".ads",
    ".advert",
    "#ad",
    ".lang-selector",
    ".language",
    "#language-selector",
    ".social",
    ".social-media",
    ".social-links",
    "#social",
    ".menu",
    ".navigation",
    "#nav",
    ".breadcrumbs",
    "#breadcrumbs",
    ".share",
    "#share",
    ".widget",
    "#widget",
    ".cookie",
    "#cookie",
    ".fc-decoration",
];

const FORCE_INCLUDE_MAIN_TAGS: [&str; 13] = [
    "#main",
    ".swoogo-cols",
    ".swoogo-text",
    ".swoogo-table-div",
    ".swoogo-space",
    ".swoogo-alert",
    ".swoogo-sponsors",
    ".swoogo-title",
    ".swoogo-tabs",
    ".swoogo-logo",
    ".swoogo-image",
    ".swoogo-button",
    ".swoogo-agenda",
];

#[derive(Deserialize, Serialize)]
pub struct TransformHtmlOptions {
    pub html: String,
    pub url: String,
    #[serde(default)]
    pub include_tags: Vec<String>,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    pub only_main_content: bool,
    pub omce_signatures: Option<Vec<String>>,
}

struct ImageSource {
    url: String,
    size: f64,
    is_x: bool,
}

pub fn transform_html(opts: TransformHtmlOptions) -> crate::Result<String> {
    transform_html_with_method(opts).map(|(html, _)| html)
}

/// [`transform_html`] plus how the main region was chosen (design doc 122 D8): `None` unless
/// `only_main_content`.
pub fn transform_html_with_method(
    opts: TransformHtmlOptions,
) -> crate::Result<(String, Option<crate::extract::Method>)> {
    let mut document = parse_html().one(opts.html.as_ref());
    let url = Url::parse(&extract_base_href_from_document(
        &document,
        &Url::parse(&opts.url)?,
    )?)?;

    if !opts.include_tags.is_empty() {
        let new_document = parse_html().one("<div></div>");
        let root = new_document
            .select_first("div")
            .map_err(|_| "Failed to select root element")?;

        for x in opts.include_tags.iter() {
            let matching_nodes: Vec<_> = document
                .select(x)
                .map_err(|_| "Failed to include_tags tags")?
                .collect();
            for tag in matching_nodes {
                root.as_node().append(tag.as_node().clone());
            }
        }

        document = new_document;
    }

    while let Ok(x) = document.select_first("head") {
        x.as_node().detach();
    }
    while let Ok(x) = document.select_first("meta") {
        x.as_node().detach();
    }
    while let Ok(x) = document.select_first("noscript") {
        x.as_node().detach();
    }
    while let Ok(x) = document.select_first("style") {
        x.as_node().detach();
    }
    while let Ok(x) = document.select_first("script") {
        x.as_node().detach();
    }

    // OMCE first
    if opts.only_main_content {
        if let Some(signatures) = opts.omce_signatures.as_ref() {
            let mut nodes_to_drop: Vec<NodeRef> = Vec::new();

            let modes = signatures
                .iter()
                .map(|x| Into::<SignatureMode>::into(x.split(':').nth(1).unwrap().to_string()))
                .collect::<HashSet<_>>();

            for mode in modes {
                let matcher = format!(":{}:", Into::<String>::into(mode));
                let signatures = signatures
                    .iter()
                    .filter(|x| x.contains(&matcher))
                    .cloned()
                    .collect::<HashSet<_>>();

                for edge in document.traverse() {
                    match edge {
                        NodeEdge::Start(_) => {}
                        NodeEdge::End(node) => {
                            if node.as_element().is_none() {
                                continue;
                            }
                            if node.text_contents().trim().is_empty() {
                                continue;
                            }

                            let signature = get_node_signature(&node, mode);
                            if signatures.contains(&signature) {
                                nodes_to_drop.push(node);
                            }
                        }
                    }
                }
            }

            for node in nodes_to_drop {
                node.detach();
            }
        }
    }

    for x in opts.exclude_tags.iter() {
        while let Ok(x) = document.select_first(x) {
            x.as_node().detach();
        }
    }

    let mut method = None;
    if opts.only_main_content {
        for x in EXCLUDE_NON_MAIN_TAGS.iter() {
            let x: Vec<_> = document
                .select(x)
                .map_err(|_| "Failed to select tags")?
                .collect();
            for tag in x {
                let node = tag.as_node();
                let forced = FORCE_INCLUDE_MAIN_TAGS
                    .iter()
                    .any(|x| node.select(x).is_ok_and(|mut x| x.next().is_some()));
                // 122 D8 safety valves: text valve, <form> rule, structural ids.
                if !forced && crate::noise::may_strip(node) {
                    node.detach();
                }
            }
        }

        // 122 D8: consent platforms by id/class prefix (a table, not more selectors).
        let consent: Vec<NodeRef> = document
            .inclusive_descendants()
            .filter(crate::noise::has_consent_prefix)
            .collect();
        for node in consent {
            if crate::noise::may_strip(&node) {
                node.detach();
            }
        }

        // 122 D8 <form> rule, made to hold end to end: a noise form goes, a content form
        // becomes a <div> so the markdown converter's form removal cannot eat the page.
        let forms: Vec<NodeRef> = document
            .select("form")
            .map_err(|_| "Failed to select forms")?
            .map(|f| f.as_node().clone())
            .collect();
        for form in forms {
            if crate::noise::is_noise(&form) {
                form.detach();
            } else {
                let div = new_element("div")?;
                while let Some(child) = form.first_child() {
                    div.append(child);
                }
                form.insert_before(div);
                form.detach();
            }
        }

        // 122 D8: main-content selection on the stripped document, against the untouched page.
        let selection = crate::extract::select_main(
            &document.to_string(),
            &opts.html,
            &crate::extract::SelectOptions {
                explicit_scope: !opts.include_tags.is_empty(),
                only_main_content: true,
            },
        );
        method = Some(selection.method);
        document = parse_html().one(selection.html);
    }

    // 122 D12 rails before the markdown converter.
    collapse_wrapper_chains(&document)?;
    unwrap_layout_tables(&document)?;
    normalise_code_languages(&document)?;

    // Resolve images, including lazy-loaded ones. Many sites ship a tiny
    // placeholder in `src` and keep the real image in `data-srcset` / `data-src`,
    // swapping it in via JS only once the image scrolls into view. When the real
    // `srcset` isn't present yet, fall back to those lazy-load attributes so we
    // emit the actual image instead of the placeholder. Images that already have
    // a `srcset` are handled exactly as before.
    let imgs: Vec<_> = document
        .select("img")
        .map_err(|_| "Failed to select images")?
        .collect();
    for img in imgs {
        let srcset = {
            let attrs = img.attributes.borrow();
            attrs
                .get("srcset")
                .filter(|x| !x.trim().is_empty())
                .or_else(|| attrs.get("data-srcset").filter(|x| !x.trim().is_empty()))
                .map(|x| x.to_string())
        };

        if let Some(srcset) = srcset {
            let mut sizes: Vec<ImageSource> = srcset
                .split(',')
                .filter_map(|x| {
                    let tok: Vec<&str> = x.trim().split(' ').collect();
                    let last_token = tok[tok.len() - 1];
                    let (last_token, last_token_used) = if tok.len() > 1
                        && !last_token.is_empty()
                        && (last_token.ends_with('x') || last_token.ends_with('w'))
                    {
                        (last_token, true)
                    } else {
                        ("1x", false)
                    };

                    if let Some((last_index, _)) = last_token.char_indices().last() {
                        if let Ok(parsed_size) = last_token[..last_index].parse() {
                            Some(ImageSource {
                                url: if last_token_used {
                                    tok[0..tok.len() - 1].join(" ")
                                } else {
                                    tok.join(" ")
                                },
                                size: parsed_size,
                                is_x: last_token.ends_with('x'),
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();

            if sizes.iter().all(|x| x.is_x) {
                if let Some(src) = img.attributes.borrow().get("src").map(|x| x.to_string()) {
                    sizes.push(ImageSource {
                        url: src,
                        size: 1.0,
                        is_x: true,
                    });
                }
            }

            sizes.sort_by(|a, b| {
                b.size
                    .partial_cmp(&a.size)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            if let Some(biggest) = sizes.first() {
                img.attributes
                    .borrow_mut()
                    .insert("src", biggest.url.clone());
            }
        } else {
            // No usable `srcset`/`data-srcset`: fall back to `data-src` for the
            // common lazy-load case where only a single image URL is deferred.
            let data_src = {
                let attrs = img.attributes.borrow();
                attrs
                    .get("data-src")
                    .filter(|x| !x.trim().is_empty())
                    .map(|x| x.to_string())
            };
            if let Some(data_src) = data_src {
                img.attributes.borrow_mut().insert("src", data_src);
            }
        }
    }

    let src_images: Vec<_> = document
        .select("img[src]")
        .map_err(|_| "Failed to select src images")?
        .collect();
    for img in src_images {
        let old = img
            .attributes
            .borrow()
            .get("src")
            .map(|x| x.to_string())
            .ok_or("Failed to get src")?;
        if let Ok(new) = url.join(&old) {
            img.attributes.borrow_mut().insert("src", new.to_string());
        }
    }

    let href_anchors: Vec<_> = document
        .select("a[href]")
        .map_err(|_| "Failed to select href anchors")?
        .collect();
    for anchor in href_anchors {
        let old = anchor
            .attributes
            .borrow()
            .get("href")
            .map(|x| x.to_string())
            .ok_or("Failed to get href")?;
        if let Ok(new) = url.join(&old) {
            anchor
                .attributes
                .borrow_mut()
                .insert("href", new.to_string());
        }
    }

    Ok((document.to_string(), method))
}

/// A fresh `<tag>` element; kuchikiki exposes no `QualName` constructor without html5ever.
fn new_element(tag: &str) -> crate::Result<NodeRef> {
    let doc = parse_html().one(format!("<{tag}></{tag}>"));
    let node = doc
        .select_first(tag)
        .map_err(|_| format!("Failed to create <{tag}>"))?
        .as_node()
        .clone();
    node.detach();
    Ok(node)
}

/// 122 D12 depth rail: a `<div>` whose only child is another `<div>` (whitespace aside) is
/// replaced by that child, so wrapper soup collapses to one level and content nested past the
/// markdown converter's recursion ceiling (html-to-markdown-rs defaults to 64) is not lost.
fn collapse_wrapper_chains(document: &NodeRef) -> crate::Result<()> {
    let divs: Vec<NodeRef> = document
        .select("div")
        .map_err(|_| "Failed to select divs")?
        .map(|d| d.as_node().clone())
        .collect();
    for div in divs {
        let mut only_child: Option<NodeRef> = None;
        let mut lone = true;
        for child in div.children() {
            if let Some(t) = child.as_text() {
                if t.borrow().trim().is_empty() {
                    continue;
                }
                lone = false;
                break;
            }
            if child.as_comment().is_some() {
                continue;
            }
            let is_div = child.as_element().is_some_and(|e| &*e.name.local == "div");
            if !is_div || only_child.is_some() {
                lone = false;
                break;
            }
            only_child = Some(child);
        }
        if let (true, Some(child)) = (lone, only_child) {
            div.insert_before(child);
            div.detach();
        }
    }
    Ok(())
}

/// Block-level content that makes a table cell a layout cell, not data.
const LAYOUT_CELL_BLOCKS: &str = "p, div, ul, ol, h1, h2, h3, h4, h5, h6, table, blockquote";

/// 122 D12 layout-table pre-pass: a `<table>` with any own cell holding block content becomes one
/// `<section>` per cell, in cell order, where the table stood. Data tables are untouched.
fn unwrap_layout_tables(document: &NodeRef) -> crate::Result<()> {
    let tables: Vec<NodeRef> = document
        .select("table")
        .map_err(|_| "Failed to select tables")?
        .map(|t| t.as_node().clone())
        .collect();
    for table in tables {
        let cells: Vec<NodeRef> = table
            .select("td, th")
            .map_err(|_| "Failed to select cells")?
            .map(|c| c.as_node().clone())
            .filter(|c| {
                c.ancestors()
                    .find(|a| a.as_element().is_some_and(|e| &*e.name.local == "table"))
                    .is_some_and(|t| t == table)
            })
            .collect();
        let layout = cells.iter().any(|c| {
            c.select(LAYOUT_CELL_BLOCKS)
                .is_ok_and(|mut it| it.next().is_some())
        });
        if !layout {
            continue;
        }
        for cell in cells {
            let section = new_element("section")?;
            while let Some(child) = cell.first_child() {
                section.append(child);
            }
            table.insert_before(section);
        }
        table.detach();
    }
    Ok(())
}

const LANG_PREFIXES: [&str; 4] = ["language-", "lang-", "highlight-", "sp-"];
const BARE_LANGS: [&str; 44] = [
    "bash",
    "c",
    "cpp",
    "cs",
    "csharp",
    "css",
    "diff",
    "dockerfile",
    "go",
    "haskell",
    "html",
    "ini",
    "java",
    "javascript",
    "js",
    "json",
    "kotlin",
    "lua",
    "makefile",
    "markdown",
    "md",
    "perl",
    "php",
    "plaintext",
    "powershell",
    "py",
    "python",
    "r",
    "ruby",
    "rust",
    "scala",
    "sh",
    "shell",
    "sql",
    "swift",
    "text",
    "toml",
    "ts",
    "typescript",
    "xml",
    "yaml",
    "yml",
    "zsh",
    "zig",
];

fn normalise_lang(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "py" => "python".to_owned(),
        "sh" | "zsh" | "shell" => "bash".to_owned(),
        "yml" => "yaml".to_owned(),
        "ts" => "typescript".to_owned(),
        _ => lower,
    }
}

/// The code language named by a class list: `language-X`, `lang-X`, `highlight-X`, `sp-X` or a
/// bare known name, normalised (`py` → `python`, `sh|zsh|shell` → `bash`, `yml` → `yaml`,
/// `ts` → `typescript`).
pub fn code_language(classes: &str) -> Option<String> {
    for tok in classes.split_whitespace() {
        for p in LANG_PREFIXES {
            if let Some(rest) = tok.strip_prefix(p) {
                if !rest.is_empty() {
                    return Some(normalise_lang(rest));
                }
            }
        }
        if BARE_LANGS.contains(&tok.to_ascii_lowercase().as_str()) {
            return Some(normalise_lang(tok));
        }
    }
    None
}

/// 122 D12: html-to-markdown-rs 3.12 reads only `language-X`/`lang-X` from `<pre>` then its first
/// `<code>` child, unnormalised; this rewrites every `<pre>` that names a language (by any of the
/// forms `code_language` knows, on the `<pre>` or its first `<code>`) to `class="language-<norm>"`.
fn normalise_code_languages(document: &NodeRef) -> crate::Result<()> {
    let pres: Vec<_> = document
        .select("pre")
        .map_err(|_| "Failed to select pre")?
        .collect();
    for pre in pres {
        let lang = pre
            .attributes
            .borrow()
            .get("class")
            .and_then(code_language)
            .or_else(|| {
                pre.as_node()
                    .children()
                    .find_map(|c| {
                        let el = c.as_element()?;
                        (&*el.name.local == "code")
                            .then(|| el.attributes.borrow().get("class").and_then(code_language))
                    })
                    .flatten()
            });
        if let Some(lang) = lang {
            pre.attributes
                .borrow_mut()
                .insert("class", format!("language-{lang}"));
        }
    }
    Ok(())
}

pub fn get_inner_json(html: &str) -> crate::Result<String> {
    parse_html()
        .one(html)
        .select_first("body")
        .map(|b| b.text_contents())
        .map_err(|_| crate::NativeError::new("Failed to get inner JSON"))
}

#[derive(Deserialize, Serialize)]
pub struct AttributeSelector {
    pub selector: String,
    pub attribute: String,
}

#[derive(Deserialize, Serialize)]
pub struct ExtractAttributesOptions {
    pub selectors: Vec<AttributeSelector>,
}

#[derive(Serialize)]
pub struct ExtractedAttributeResult {
    pub selector: String,
    pub attribute: String,
    pub values: Vec<String>,
}

pub fn extract_attributes(
    html: &str,
    options: &ExtractAttributesOptions,
) -> crate::Result<Vec<ExtractedAttributeResult>> {
    let document = parse_html().one(html);
    let mut results = Vec::new();

    for selector_config in &options.selectors {
        let mut values = Vec::new();

        let elements: Vec<_> = match document.select(&selector_config.selector).map_err(|_| {
            format!(
                "Failed to select with selector: {}",
                selector_config.selector
            )
        }) {
            Ok(x) => x.collect(),
            Err(_) => Vec::new(), // invalid selector => empty list
        };

        for element in elements {
            if let Some(attr_value) = element
                .attributes
                .borrow()
                .get(selector_config.attribute.as_str())
            {
                values.push(attr_value.to_string());
                continue;
            }

            if !selector_config.attribute.starts_with("data-") {
                let data_attr = format!("data-{}", selector_config.attribute);
                if let Some(attr_value) = element.attributes.borrow().get(data_attr.as_str()) {
                    values.push(attr_value.to_string());
                }
            }
        }

        results.push(ExtractedAttributeResult {
            selector: selector_config.selector.clone(),
            attribute: selector_config.attribute.clone(),
            values,
        });
    }

    Ok(results)
}

pub fn extract_images(html: &str, base_url: &str) -> crate::Result<Vec<String>> {
    let document = parse_html().one(html);
    let base_url = Url::parse(base_url)?;
    let base_href = extract_base_href_from_document(&document, &base_url)?;
    let base_href_url = Url::parse(&base_href)?;
    let mut images = HashSet::<String>::new();

    let resolve_image_url = |src: &str| -> crate::Result<String> {
        if src.starts_with("data:") || src.starts_with("blob:") {
            return Ok(src.to_string());
        }
        if src.starts_with("http://") || src.starts_with("https://") {
            return Ok(src.to_string());
        }
        if src.starts_with("//") {
            let resolved = base_url.join(src)?;
            return Ok(resolved.to_string());
        }
        let resolved = base_href_url.join(src)?;
        Ok(resolved.to_string())
    };

    // <img>
    let img_elements: Vec<_> = document
        .select("img")
        .map_err(|_| "Failed to select img tags")?
        .collect();

    for img in img_elements {
        let attrs = img.attributes.borrow();

        if let Some(src) = attrs.get("src") {
            if let Ok(resolved) = resolve_image_url(src) {
                images.insert(resolved);
            }
        }

        if let Some(data_src) = attrs.get("data-src") {
            if let Ok(resolved) = resolve_image_url(data_src) {
                images.insert(resolved);
            }
        }

        if let Some(srcset) = attrs.get("srcset") {
            for part in srcset.split(',') {
                if let Some(url) = part.split_whitespace().next() {
                    if !url.is_empty() {
                        if let Ok(resolved) = resolve_image_url(url) {
                            images.insert(resolved);
                        }
                    }
                }
            }
        }
    }

    // <picture><source>
    let source_elements: Vec<_> = match document
        .select("picture source")
        .map_err(|_| "Failed to select picture source")
    {
        Ok(x) => x.collect(),
        Err(_) => Vec::new(),
    };

    for source in source_elements {
        if let Some(srcset) = source.attributes.borrow().get("srcset") {
            for part in srcset.split(',') {
                if let Some(url) = part.split_whitespace().next() {
                    if !url.is_empty() {
                        if let Ok(resolved) = resolve_image_url(url) {
                            images.insert(resolved);
                        }
                    }
                }
            }
        }
    }

    // OG/Twitter images
    let meta_selectors = [
        "meta[property=\"og:image\"]",
        "meta[property=\"og:image:url\"]",
        "meta[property=\"og:image:secure_url\"]",
        "meta[name=\"twitter:image\"]",
        "meta[name=\"twitter:image:src\"]",
        "meta[itemprop=\"image\"]",
    ];

    for selector in &meta_selectors {
        if let Ok(elements) = document.select(selector) {
            for element in elements {
                if let Some(content) = element.attributes.borrow().get("content") {
                    if let Ok(resolved) = resolve_image_url(content) {
                        images.insert(resolved);
                    }
                }
            }
        }
    }

    // icons
    let link_selectors = [
        "link[rel*=\"icon\"]",
        "link[rel*=\"apple-touch-icon\"]",
        "link[rel*=\"image_src\"]",
    ];

    for selector in &link_selectors {
        if let Ok(elements) = document.select(selector) {
            for element in elements {
                if let Some(href) = element.attributes.borrow().get("href") {
                    if let Ok(resolved) = resolve_image_url(href) {
                        images.insert(resolved);
                    }
                }
            }
        }
    }

    // <video poster="">
    if let Ok(video_elements) = document.select("video[poster]") {
        for video in video_elements {
            if let Some(poster) = video.attributes.borrow().get("poster") {
                if let Ok(resolved) = resolve_image_url(poster) {
                    images.insert(resolved);
                }
            }
        }
    }

    // <... style="background: url(...)"> or <... style="background-image: url(...)">
    if let Ok(elements) = document.select("[style*=\"background\"]") {
        for element in elements {
            if let Some(style) = element.attributes.borrow().get("style") {
                for cap in URL_REGEX.captures_iter(style) {
                    if let Some(url_match) = cap.get(1) {
                        let url = url_match.as_str().trim();
                        if !url.is_empty() {
                            if let Ok(resolved) = resolve_image_url(url) {
                                images.insert(resolved);
                            }
                        }
                    }
                }
            }
        }
    }

    let filtered_images: Vec<String> = images
        .into_iter()
        .filter(|url| !url.to_lowercase().starts_with("javascript:"))
        .filter(|url| !url.is_empty())
        .filter(|url| {
            url.starts_with("data:") || url.starts_with("blob:") || Url::parse(url).is_ok()
        })
        .collect();

    Ok(filtered_images)
}

/// Process multi-line links in markdown.
pub fn post_process_markdown(markdown: String) -> String {
    let mut link_open_count = 0usize;
    let mut out = String::with_capacity(markdown.len());

    for ch in markdown.chars() {
        match ch {
            '[' => {
                link_open_count += 1;
            }
            ']' => {
                link_open_count = link_open_count.saturating_sub(1);
            }
            _ => {}
        }

        let inside_link_content = link_open_count > 0;
        if inside_link_content && ch == '\n' {
            out.push('\\');
            out.push('\n');
        } else {
            out.push(ch);
        }
    }

    remove_skip_to_content_links(&out)
}

fn remove_skip_to_content_links(input: &str) -> String {
    const LABEL: &str = "Skip to Content";
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;

    'outer: while i < len {
        if bytes[i] == b'[' {
            let label_start = i + 1;
            let label_end = label_start + LABEL.len();

            if label_end <= len && bytes[label_start..label_end].iter().all(|b| b.is_ascii()) {
                let label_slice = &input[label_start..label_end];

                if label_slice.eq_ignore_ascii_case(LABEL)
                    && label_end + 3 <= len
                    && bytes[label_end] == b']'
                    && bytes[label_end + 1] == b'('
                    && bytes[label_end + 2] == b'#'
                {
                    let mut j = label_end + 3;

                    while j < len {
                        let ch = input[j..].chars().next().unwrap();
                        if ch == ')' {
                            i = j + ch.len_utf8();
                            continue 'outer;
                        }
                        j += ch.len_utf8();
                    }
                }
            }
        }

        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

#[cfg(test)]
mod tests_122 {
    //! Design doc 122 D8/D12: the hook and the rails in `transform_html`.
    use super::*;

    fn transform(html: &str, only_main_content: bool) -> (String, Option<crate::extract::Method>) {
        transform_html_with_method(TransformHtmlOptions {
            html: html.to_owned(),
            url: "https://example.test/".to_owned(),
            include_tags: Vec::new(),
            exclude_tags: Vec::new(),
            only_main_content,
            omce_signatures: None,
        })
        .expect("transform")
    }

    #[test]
    fn code_language_table() {
        assert_eq!(code_language("language-rust").as_deref(), Some("rust"));
        assert_eq!(code_language("lang-py").as_deref(), Some("python"));
        assert_eq!(code_language("highlight-Sh").as_deref(), Some("bash"));
        assert_eq!(code_language("sp-yml").as_deref(), Some("yaml"));
        assert_eq!(code_language("hljs ts").as_deref(), Some("typescript"));
        assert_eq!(code_language("highlight zsh").as_deref(), Some("bash"));
        assert_eq!(code_language("js").as_deref(), Some("js"));
        assert_eq!(code_language("highlight codeblock"), None);
        assert_eq!(code_language("language-"), None);
    }

    #[test]
    fn pre_classes_are_rewritten_for_the_converter() {
        let html = r#"<body><pre class="highlight"><code class="sh">ls</code></pre><pre class="lang-rust"><code>fn</code></pre><pre><code class="language-py">x</code></pre><pre><code>plain</code></pre></body>"#;
        let (out, _) = transform(html, false);
        assert!(out.contains(r#"<pre class="language-bash"><code class="sh">ls</code></pre>"#));
        assert!(out.contains(r#"<pre class="language-rust"><code>fn</code></pre>"#));
        assert!(
            out.contains(
                r#"<pre class="language-python"><code class="language-py">x</code></pre>"#
            )
        );
        assert!(out.contains("<pre><code>plain</code></pre>"));
    }

    #[test]
    fn layout_tables_become_sections_data_tables_stay() {
        let html = r#"<body><table><tr><td><p>left</p></td><td><ul><li>a</li></ul></td></tr></table><table><tr><th>k</th><th>v</th></tr><tr><td>1</td><td>2</td></tr></table></body>"#;
        let (out, _) = transform(html, false);
        assert!(
            out.contains("<section><p>left</p></section><section><ul><li>a</li></ul></section>")
        );
        assert_eq!(out.matches("<table>").count(), 1);
        assert!(out.contains("<th>k</th>"));
    }

    #[test]
    fn nested_layout_table_keeps_inner_data_table() {
        let html = r#"<body><table><tr><td><table><tr><td>1</td><td>2</td></tr></table></td><td><p>side</p></td></tr></table></body>"#;
        let (out, _) = transform(html, false);
        assert_eq!(out.matches("<table>").count(), 1);
        assert!(out.contains("<section><table>"));
        assert!(out.contains("<section><p>side</p></section>"));
    }

    #[test]
    fn forms_follow_the_form_rule_only_when_only_main_content() {
        let long = "word ".repeat(130);
        let html = format!(
            r#"<body><form id="aspnetForm"><div class="content"><h1>T</h1><p>{long}</p></div></form><form class="login-form"><input name="u"></form></body>"#
        );
        let (out, method) = transform(&html, true);
        assert!(!out.contains("<form"));
        assert!(out.contains("<h1>T</h1>"));
        assert_eq!(method, Some(crate::extract::Method::Scored));
        let (out, method) = transform(&html, false);
        assert_eq!(out.matches("<form").count(), 2);
        assert_eq!(method, None);
    }

    #[test]
    fn consent_overlays_stripped_and_valves_hold() {
        let long = "word ".repeat(1_100);
        let html = format!(
            r#"<body><div id="onetrust-consent-sdk"><p>We value your privacy</p></div><div class="cc-window">cookies</div><div class="header"><nav><a href="/">x</a></nav><div class="post"><h1>T</h1><p>{long}</p></div></div></body>"#
        );
        let (out, method) = transform(&html, true);
        assert!(!out.contains("We value your privacy"));
        assert!(!out.contains("cookies"));
        assert!(!out.contains("<nav>"));
        assert!(out.contains("<h1>T</h1>"));
        assert_eq!(method, Some(crate::extract::Method::Scored));
    }

    #[test]
    fn main_wins_and_srcset_largest_is_chosen() {
        let html = r#"<body><aside>side</aside><main><p>text</p><img srcset="/a-480.jpg 480w, /a-1600.jpg 1600w, /a-960.jpg 960w" alt="a"></main></body>"#;
        let (out, method) = transform(html, true);
        assert_eq!(method, Some(crate::extract::Method::Main));
        assert!(!out.contains("side"));
        assert!(out.contains(r#"src="https://example.test/a-1600.jpg""#));
    }
}

#[cfg(test)]
mod tests_122_depth {
    use super::*;

    #[test]
    fn wrapper_chains_collapse_but_siblings_stay() {
        let open = "<div class=\"l\">".repeat(260);
        let close = "</div>".repeat(260);
        let html = format!(
            "<body>{open}<h1>T</h1><p>deep</p>{close}<div><div>a</div><div>b</div></div><div> <!-- c --> <div><span>x</span></div></div></body>"
        );
        let (out, _) = transform_html_with_method(TransformHtmlOptions {
            html,
            url: "https://example.test/".to_owned(),
            include_tags: Vec::new(),
            exclude_tags: Vec::new(),
            only_main_content: false,
            omce_signatures: None,
        })
        .expect("transform");
        assert!(out.contains("<body><div class=\"l\"><h1>T</h1><p>deep</p></div>"));
        assert!(out.contains("<div><div>a</div><div>b</div></div>"));
        assert!(out.contains("<div><span>x</span></div>"));
        assert_eq!(out.matches("<div").count(), 5);
    }
}
