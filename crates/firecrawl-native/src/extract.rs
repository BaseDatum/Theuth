//! Main-content selection (design doc 122 D8): `<main>`/`<article>`/`[role=main]` first,
//! else a scored candidate, else `<body>`; `<noscript>` recovery against the untouched page;
//! two invariants — an explicit scope (or a real `<main>`) is final, and a thin unscoped
//! result retries with `<body>`.

use kuchikiki::{NodeRef, iter::NodeEdge, parse_html, traits::TendrilSink};
use serde::Serialize;

use crate::noise::{collapsed_len, is_noise};

/// Elements nested deeper than this are not candidates; their text still counts (D12).
pub const MAX_DEPTH: usize = 200;
/// Below this many text chars `ln(text_len)` contributes 0.
const MIN_TEXT_CHARS: usize = 50;
const BONUS_SEMANTIC_TAG: f64 = 50.0;
const BONUS_ROLE_MAIN: f64 = 50.0;
const BONUS_CLASS: f64 = 25.0;
const BONUS_ID: f64 = 25.0;
const PER_PARAGRAPH: f64 = 3.0;
const CLASS_HINTS: [&str; 4] = ["content", "article", "post", "entry"];
const ID_HINTS: [&str; 4] = ["content", "article", "post", "main"];
const CANDIDATE_TAGS: [&str; 5] = ["article", "main", "div", "section", "td"];
/// `<noscript>` recovery runs when the selection has fewer words than this …
const NOSCRIPT_THIN_WORDS: usize = 30;
/// … and a `<noscript>` is used when it yields at least this many words (and more than the selection).
const NOSCRIPT_MIN_WORDS: usize = 5;
/// Invariant 2: an unscoped result under this many words …
const BODY_RETRY_WORDS: usize = 200;
/// … is replaced by `<body>` when the body has > 2× the words and more than this many.
const BODY_RETRY_MIN_WORDS: usize = 50;

/// How the main region was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// `<main>` or `[role=main]`.
    Main,
    /// First `<article>`.
    Article,
    /// Highest-scoring candidate.
    Scored,
    /// `<body>` (no candidate, or the body retry).
    Body,
}

/// Caller intent for [`select_main`].
#[derive(Clone, Debug, Default)]
pub struct SelectOptions {
    /// The caller passed `include_tags` (explicit scope): never widen.
    pub explicit_scope: bool,
    pub only_main_content: bool,
}

/// The chosen region.
#[derive(Clone, Debug)]
pub struct Selection {
    /// Serialised HTML of the chosen region (or the whole stripped document).
    pub html: String,
    /// `true` when a `<main>`/`<article>`/`[role=main]` or explicit scope decided.
    pub scoped: bool,
    pub method: Method,
}

/// What the scorer knows about one candidate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CandidateStats {
    /// Text chars (whitespace collapsed).
    pub text_len: usize,
    /// Text chars inside `<a>` descendants.
    pub anchor_len: usize,
    /// `<p>` descendants (within the depth guard).
    pub p_count: usize,
    /// Tag is `article` or `main`.
    pub semantic_tag: bool,
    /// `role="main"`.
    pub role_main: bool,
    /// Class attribute contains `content|article|post|entry`.
    pub class_hint: bool,
    /// Id attribute contains `content|article|post|main`.
    pub id_hint: bool,
}

/// Share of `text_len` inside anchors (`0.0` for empty text).
#[must_use]
pub fn link_density(anchor_len: usize, text_len: usize) -> f64 {
    if text_len == 0 {
        return 0.0;
    }
    anchor_len as f64 / text_len as f64
}

/// D8's formula: `ln(text_len)` (0 below 50), plus 50 for a semantic tag, 50 for `role=main`,
/// 25 for a class hint, 25 for an id hint and 3 per paragraph, times the link-density penalty
/// (semantic: ×0.3 above 0.7, ×0.5 above 0.5; generic: ×0.1 above 0.5, ×0.5 above 0.3).
#[must_use]
pub fn score(c: &CandidateStats) -> f64 {
    let mut s = if c.text_len < MIN_TEXT_CHARS {
        0.0
    } else {
        (c.text_len as f64).ln()
    };
    if c.semantic_tag {
        s += BONUS_SEMANTIC_TAG;
    }
    if c.role_main {
        s += BONUS_ROLE_MAIN;
    }
    if c.class_hint {
        s += BONUS_CLASS;
    }
    if c.id_hint {
        s += BONUS_ID;
    }
    s += PER_PARAGRAPH * c.p_count as f64;
    let d = link_density(c.anchor_len, c.text_len);
    let penalty = if c.semantic_tag || c.role_main {
        if d > 0.7 {
            0.3
        } else if d > 0.5 {
            0.5
        } else {
            1.0
        }
    } else if d > 0.5 {
        0.1
    } else if d > 0.3 {
        0.5
    } else {
        1.0
    };
    s * penalty
}

struct Frame {
    text: usize,
    anchor: usize,
    p: usize,
    start: usize,
    is_anchor: bool,
    noise: bool,
}

struct Candidate {
    start: usize,
    score: f64,
    node: NodeRef,
}

fn attr_contains(attrs: &kuchikiki::Attributes, name: &str, hints: &[&str]) -> bool {
    attrs.get(name).is_some_and(|v| {
        let v = v.to_ascii_lowercase();
        hints.iter().any(|h| v.contains(h))
    })
}

/// One pass over `root`: per-element text/anchor/paragraph totals bubble up to the parent at
/// `End`; every non-noise candidate tag at depth ≤ [`MAX_DEPTH`] outside noise is scored.
fn collect_candidates(root: &NodeRef) -> Vec<Candidate> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut candidates = Vec::new();
    let mut noise_depth = 0usize;
    let mut anchor_depth = 0usize;
    let mut order = 0usize;
    for edge in root.traverse_inclusive() {
        match edge {
            NodeEdge::Start(node) => {
                if let Some(text) = node.as_text() {
                    if let Some(top) = stack.last_mut() {
                        let len = collapsed_len(&text.borrow());
                        top.text += len;
                        if anchor_depth > 0 {
                            top.anchor += len;
                        }
                    }
                } else if let Some(el) = node.as_element() {
                    let noise = stack.len() <= MAX_DEPTH && is_noise(&node);
                    if noise {
                        noise_depth += 1;
                    }
                    let is_anchor = &*el.name.local == "a";
                    if is_anchor {
                        anchor_depth += 1;
                    }
                    stack.push(Frame {
                        text: 0,
                        anchor: 0,
                        p: 0,
                        start: order,
                        is_anchor,
                        noise,
                    });
                    order += 1;
                }
            }
            NodeEdge::End(node) => {
                let Some(el) = node.as_element() else {
                    continue;
                };
                let Some(frame) = stack.pop() else {
                    continue;
                };
                if frame.is_anchor {
                    anchor_depth -= 1;
                }
                if frame.noise {
                    noise_depth -= 1;
                }
                let depth = stack.len();
                let tag: &str = &el.name.local;
                let attrs = el.attributes.borrow();
                let role_main = attrs
                    .get("role")
                    .is_some_and(|r| r.trim().eq_ignore_ascii_case("main"));
                if depth <= MAX_DEPTH
                    && noise_depth == 0
                    && !frame.noise
                    && (role_main || CANDIDATE_TAGS.contains(&tag))
                {
                    let stats = CandidateStats {
                        text_len: frame.text,
                        anchor_len: frame.anchor,
                        p_count: frame.p,
                        semantic_tag: tag == "article" || tag == "main",
                        role_main,
                        class_hint: attr_contains(&attrs, "class", &CLASS_HINTS),
                        id_hint: attr_contains(&attrs, "id", &ID_HINTS),
                    };
                    let s = score(&stats);
                    if s > 0.0 {
                        candidates.push(Candidate {
                            start: frame.start,
                            score: s,
                            node: node.clone(),
                        });
                    }
                }
                if let Some(parent) = stack.last_mut() {
                    parent.text += frame.text;
                    parent.anchor += frame.anchor;
                    // D12 depth guard: deeper subtrees count as text only.
                    if depth <= MAX_DEPTH {
                        parent.p += frame.p + usize::from(tag == "p");
                    }
                }
            }
        }
    }
    candidates
}

/// The best candidate: highest score, ties by document order.
fn best_candidate(doc: &NodeRef) -> Option<NodeRef> {
    collect_candidates(doc)
        .into_iter()
        .fold(None::<Candidate>, |best, c| match best {
            Some(b) if b.score > c.score || (b.score == c.score && b.start < c.start) => Some(b),
            _ => Some(c),
        })
        .map(|c| c.node)
}

fn words(node: &NodeRef) -> usize {
    node.text_contents().split_whitespace().count()
}

/// `(tag, id, class)` — enough to find the same element again in the untouched page.
fn element_key(node: &NodeRef) -> Option<(String, Option<String>, Option<String>)> {
    let el = node.as_element()?;
    let attrs = el.attributes.borrow();
    Some((
        el.name.local.to_string(),
        attrs.get("id").map(str::to_owned),
        attrs.get("class").map(str::to_owned),
    ))
}

/// Inner markup of a `<noscript>`: with scripting enabled the parser keeps it as raw text,
/// otherwise as elements — both serialise to HTML here.
fn noscript_inner(node: &NodeRef) -> String {
    let mut out = String::new();
    for child in node.children() {
        if let Some(t) = child.as_text() {
            out.push_str(&t.borrow());
        } else {
            out.push_str(&child.to_string());
        }
    }
    out
}

/// Step 4: when the selection has < 30 words, each `<noscript>` of `original` under the selected
/// element is re-parsed as a `<section>` and appended when it yields ≥ 5 words and more words
/// than the selection has.
fn recover_noscript(selection: &mut Selection, selected: &NodeRef, original: &str) {
    let mut current = words(selected);
    if current >= NOSCRIPT_THIN_WORDS {
        return;
    }
    let key = element_key(selected);
    let is_body = key.as_ref().is_some_and(|k| k.0 == "body");
    let orig = parse_html().one(original);
    let Ok(noscripts) = orig.select("noscript") else {
        return;
    };
    for ns in noscripts {
        let node = ns.as_node();
        if !is_body
            && !node
                .ancestors()
                .any(|a| element_key(&a).as_ref() == key.as_ref())
        {
            continue;
        }
        let frag = parse_html().one(format!("<section>{}</section>", noscript_inner(node)));
        let Ok(section) = frag.select_first("section") else {
            continue;
        };
        let w = words(section.as_node());
        if w >= NOSCRIPT_MIN_WORDS && w > current {
            selection.html.push_str(&section.as_node().to_string());
            current = w;
        }
    }
}

/// Run after the strip list on the already-stripped document; `original` is the untouched page
/// (for `<noscript>` recovery, which `transform_html` detaches early).
///
/// 1. `<main>` → `Main`; else `<article>` → `Article`; else `[role=main]` → `Main` (all scoped).
/// 2. Else the best-scoring `article, main, [role=main], div, section, td` → `Scored`.
/// 3. Else `<body>` → `Body`.
/// 4. `<noscript>` recovery (not after `<main>`).
/// 5. Invariant 1: `explicit_scope` or a real `<main>` is final — the input (or the `<main>`) is
///    returned as is, even when empty.
/// 6. Invariant 2: an unscoped result under 200 words yields to `<body>` when the body has more
///    than twice the words and more than 50.
///
/// With `only_main_content: false` the input is returned unchanged (`Body`, unscoped).
#[must_use]
pub fn select_main(html: &str, original: &str, opts: &SelectOptions) -> Selection {
    if !opts.only_main_content {
        return Selection {
            html: html.to_owned(),
            scoped: false,
            method: Method::Body,
        };
    }
    let doc = parse_html().one(html);
    if let Ok(main) = doc.select_first("main") {
        return Selection {
            html: main.as_node().to_string(),
            scoped: true,
            method: Method::Main,
        };
    }
    if opts.explicit_scope {
        return Selection {
            html: html.to_owned(),
            scoped: true,
            method: Method::Body,
        };
    }
    let body = doc
        .select_first("body")
        .map_or_else(|()| doc.clone(), |b| b.as_node().clone());
    let (node, scoped, method) = if let Ok(article) = doc.select_first("article") {
        (article.as_node().clone(), true, Method::Article)
    } else if let Ok(role) = doc.select_first("[role=main]") {
        (role.as_node().clone(), true, Method::Main)
    } else if let Some(best) = best_candidate(&doc) {
        (best, false, Method::Scored)
    } else {
        (body.clone(), false, Method::Body)
    };
    let mut selection = Selection {
        html: node.to_string(),
        scoped,
        method,
    };
    recover_noscript(&mut selection, &node, original);
    if !scoped && method != Method::Body {
        let w = words(&node);
        let bw = words(&body);
        if w < BODY_RETRY_WORDS && bw > 2 * w && bw > BODY_RETRY_MIN_WORDS {
            selection = Selection {
                html: body.to_string(),
                scoped: false,
                method: Method::Body,
            };
            recover_noscript(&mut selection, &body, original);
        }
    }
    selection
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SelectOptions {
        SelectOptions {
            explicit_scope: false,
            only_main_content: true,
        }
    }

    fn filler(words: usize) -> String {
        (0..words)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn stats(text_len: usize) -> CandidateStats {
        CandidateStats {
            text_len,
            ..CandidateStats::default()
        }
    }

    #[test]
    fn score_matches_formula() {
        let base = (400f64).ln();
        assert!((score(&stats(400)) - base).abs() < 1e-9);
        assert_eq!(score(&stats(49)), 0.0);
        let full = CandidateStats {
            text_len: 400,
            anchor_len: 0,
            p_count: 4,
            semantic_tag: true,
            role_main: true,
            class_hint: true,
            id_hint: true,
        };
        assert!((score(&full) - (base + 50.0 + 50.0 + 25.0 + 25.0 + 12.0)).abs() < 1e-9);
    }

    #[test]
    fn density_penalties() {
        // Shared vector with `web::excerpt::link_density`'s thresholds: 0.3 / 0.5 / 0.7.
        assert!((link_density(5, 15) - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(link_density(0, 0), 0.0);
        let base = (1000f64).ln();
        let generic = |anchor| {
            score(&CandidateStats {
                text_len: 1000,
                anchor_len: anchor,
                ..CandidateStats::default()
            })
        };
        assert!((generic(300) - base).abs() < 1e-9);
        assert!((generic(301) - base * 0.5).abs() < 1e-9);
        assert!((generic(501) - base * 0.1).abs() < 1e-9);
        let semantic = |anchor| {
            score(&CandidateStats {
                text_len: 1000,
                anchor_len: anchor,
                semantic_tag: true,
                ..CandidateStats::default()
            })
        };
        assert!((semantic(500) - (base + 50.0)).abs() < 1e-9);
        assert!((semantic(501) - (base + 50.0) * 0.5).abs() < 1e-9);
        assert!((semantic(701) - (base + 50.0) * 0.3).abs() < 1e-9);
    }

    #[test]
    fn scored_candidate_wins_by_class_hint_and_paragraphs() {
        let text = filler(60);
        let html = format!(
            r#"<body><div class="col"><p>{text}</p></div><div class="post-body"><p>{text}</p><p>{text}</p></div></body>"#
        );
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Scored);
        assert!(!sel.scoped);
        assert!(sel.html.starts_with(r#"<div class="post-body">"#));
    }

    #[test]
    fn ties_keep_document_order() {
        let text = filler(80);
        let html = format!(r#"<body><div><p>{text}</p></div><div><p>{text}</p></div></body>"#);
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Scored);
        let doc = parse_html().one(html.as_str());
        let first = doc.select_first("div").unwrap().as_node().to_string();
        assert_eq!(sel.html, first);
        // Both divs serialise identically; ordering is pinned by the candidate fold.
        let c = collect_candidates(&doc);
        assert_eq!(c.len(), 2);
        assert!(c[0].start < c[1].start);
        assert_eq!(c[0].score, c[1].score);
    }

    #[test]
    fn semantic_regions_first() {
        let text = filler(300);
        let html = format!(
            r#"<body><div class="content"><p>{text}</p></div><main><p>x</p></main></body>"#
        );
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Main);
        assert!(sel.scoped);
        assert_eq!(sel.html, "<main><p>x</p></main>");
        let html = format!(
            r#"<body><div class="content"><p>{text}</p></div><article><p>y</p></article></body>"#
        );
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Article);
        let html = format!(
            r#"<body><div class="content"><p>{text}</p></div><div role="main"><p>z</p></div></body>"#
        );
        let sel = select_main(&html, &html, &opts());
        assert_eq!((sel.method, sel.scoped), (Method::Main, true));
        assert_eq!(sel.html, r#"<div role="main"><p>z</p></div>"#);
    }

    #[test]
    fn invariant_1_explicit_scope_is_final() {
        let html = "<div></div>";
        let sel = select_main(
            html,
            "<body><p>some words outside the scope</p><noscript><p>a b c d e f g</p></noscript></body>",
            &SelectOptions {
                explicit_scope: true,
                only_main_content: true,
            },
        );
        assert!(sel.scoped);
        assert_eq!(sel.html, html);
        // A real <main> with 0 words is returned empty, never widened to the body.
        let text = filler(400);
        let html = format!("<body><main></main><div class=\"content\"><p>{text}</p></div></body>");
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Main);
        assert_eq!(sel.html, "<main></main>");
    }

    #[test]
    fn invariant_2_body_retry() {
        // 190 words scored vs 400 in the body → body.
        let html = format!(
            "<body><div class=\"post\"><p>{}</p></div><p>{}</p></body>",
            filler(190),
            filler(210)
        );
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Body);
        assert!(!sel.scoped);
        assert!(sel.html.starts_with("<body>"));
        // 190 vs 300 → not.
        let html = format!(
            "<body><div class=\"post\"><p>{}</p></div><p>{}</p></body>",
            filler(190),
            filler(110)
        );
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Scored);
        // Scoped results never retry.
        let html = format!(
            "<body><article><p>{}</p></article><p>{}</p></body>",
            filler(190),
            filler(210)
        );
        assert_eq!(select_main(&html, &html, &opts()).method, Method::Article);
    }

    #[test]
    fn no_candidate_is_body() {
        let html = "<body><h1>Just a moment</h1><p>Enable JavaScript and cookies to continue.</p><div id=\"challenge-stage\"></div></body>";
        let sel = select_main(html, html, &opts());
        assert_eq!(sel.method, Method::Body);
        assert!(sel.html.contains("Enable JavaScript"));
    }

    #[test]
    fn noscript_recovery_thresholds() {
        let stripped = "<body><div class=\"post\"><p>short post text here now</p></div></body>";
        let original = "<body><div class=\"post\"><p>short post text here now</p><noscript><p>one two three four five six seven</p></noscript></div></body>";
        let sel = select_main(stripped, original, &opts());
        assert_eq!(sel.method, Method::Scored);
        assert!(
            sel.html
                .contains("<section><p>one two three four five six seven</p></section>")
        );
        // Fewer words than the selection → not used.
        let original = "<body><div class=\"post\"><p>short post text here now</p><noscript><p>one two three four</p></noscript></div></body>";
        assert!(
            !select_main(stripped, original, &opts())
                .html
                .contains("<section>")
        );
        // Under 5 words → not used even if more than the selection.
        let stripped2 = "<body><div class=\"post-body\"><p>one two three</p></div></body>";
        let original2 = "<body><div class=\"post-body\"><p>one two three</p><noscript>a b c d</noscript></div></body>";
        assert!(
            !select_main(stripped2, original2, &opts())
                .html
                .contains("<section>")
        );
        // A noscript outside the selected element is ignored.
        let original3 = "<body><div class=\"post\"><p>short post text here now</p></div><div class=\"other\"><noscript><p>one two three four five six seven</p></noscript></div></body>";
        assert!(
            !select_main(stripped, original3, &opts())
                .html
                .contains("<section>")
        );
        // A selection with ≥ 30 words never recovers.
        let text = filler(40);
        let stripped4 = format!("<body><div class=\"post\"><p>{text}</p></div></body>");
        let original4 = format!(
            "<body><div class=\"post\"><p>{text}</p><noscript><p>{}</p></noscript></div></body>",
            filler(60)
        );
        assert!(
            !select_main(&stripped4, &original4, &opts())
                .html
                .contains("<section>")
        );
        // Only-main-content off → identity.
        let sel = select_main(stripped, original, &SelectOptions::default());
        assert_eq!(sel.html, stripped);
        assert_eq!(sel.method, Method::Body);
    }

    #[test]
    fn depth_guard_200() {
        let open: String = "<div>".repeat(260);
        let close: String = "</div>".repeat(260);
        let text = filler(300);
        let html = format!("<body>{open}<p>{text}</p>{close}</body>");
        let doc = parse_html().one(html.as_str());
        let cands = collect_candidates(&doc);
        // html is depth 0, body 1, the k-th div k+1; only divs at depth ≤ 200 are candidates.
        assert_eq!(cands.len(), MAX_DEPTH - 1);
        // The paragraph is below the guard: counted as text, not as a <p>.
        let base = (collapsed_len(&text) as f64).ln();
        for c in &cands {
            assert!(
                (c.score - base).abs() < 1e-9,
                "score {} != ln(text)",
                c.score
            );
        }
        let sel = select_main(&html, &html, &opts());
        assert_eq!(sel.method, Method::Scored);
        assert!(sel.html.contains("w299"));
    }

    #[test]
    fn noise_and_inside_noise_are_not_candidates() {
        let text = filler(100);
        let html = format!(
            r#"<body><nav><div class="x"><p>{text}</p></div></nav><div class="sidebar"><p>{text}</p></div><div class="main-col"><p>{text}</p></div></body>"#
        );
        let doc = parse_html().one(html.as_str());
        let cands = collect_candidates(&doc);
        assert_eq!(cands.len(), 1);
        assert!(
            cands[0]
                .node
                .to_string()
                .starts_with(r#"<div class="main-col">"#)
        );
    }
}
