//! Tool text (design doc 16 §2, §4.4): a `Searched …` header, one `## n. title`
//! / URL section per hit with the excerpts of the pages that were read (the
//! domain line ends ` · rendered` when a browser produced them), then
//! `Features:` blocks in [`FeatureKind::ORDER`] each capped at
//! `feature_max_chars`, then one `Failed:` line. The desktop and TUI parse the
//! header/URL pairs, so the line shapes here are a contract. The same data
//! goes out as JSON in `ToolOutput::structured`; this is the model-facing half.

use std::fmt::Write as _;

use crate::aggregate::SearchResponse;
use crate::fetch::FetchResponse;
use crate::provider::{Feature, FeatureKind};

/// Cut `s` to `max` chars, appending `…` when anything was dropped.
fn cap(s: &str, max: usize) -> String {
    let mut it = s.char_indices();
    match it.nth(max) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s.to_owned(),
    }
}

fn feature_block(f: &Feature) -> String {
    let mut s = format!("- [{}]", f.kind.as_str());
    if let Some(t) = f.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = write!(s, " {t}");
    }
    if let Some(t) = f.text.as_deref().filter(|t| !t.trim().is_empty()) {
        s.push_str(if f.title.is_some() { ": " } else { " " });
        s.push_str(t.trim());
    }
    for (k, v) in &f.attributes {
        let _ = write!(s, "\n  {k}: {v}");
    }
    for item in &f.items {
        let _ = write!(s, "\n  - {}", item.title.trim());
        if !item.text.trim().is_empty() {
            let _ = write!(s, ": {}", item.text.trim());
        }
        if let Some(d) = &item.date {
            let _ = write!(s, " ({d})");
        }
        if let Some(u) = &item.url {
            let _ = write!(s, " {u}");
        }
    }
    for l in &f.links {
        let _ = write!(s, "\n  {} {}", l.title.trim(), l.url);
    }
    if let Some(img) = &f.image {
        let _ = write!(s, "\n  image: {img}");
    }
    s
}

/// Cap on the whole `search_web` text; over it, excerpts of the last read
/// pages are dropped first.
pub const SEARCH_MAX_CHARS: usize = 60_000;

/// Host of `url` without a leading `www.`; the URL itself when it does not parse.
fn domain(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_else(|| url.to_owned())
}

/// The `search_web` text: header, ranked sections with excerpts for the hits
/// in `resp.pages`, features, failures; at most [`SEARCH_MAX_CHARS`].
#[must_use]
pub fn search_text(resp: &SearchResponse, feature_max_chars: usize) -> String {
    let mut keep = resp.pages.len();
    loop {
        let s = render_search(resp, feature_max_chars, keep);
        if keep == 0 || s.chars().count() <= SEARCH_MAX_CHARS {
            return s;
        }
        keep -= 1;
    }
}

/// `keep`: how many of `resp.pages` (rank order) show their excerpts; the rest
/// get an `[N excerpts omitted]` line instead.
fn render_search(resp: &SearchResponse, feature_max_chars: usize, keep: usize) -> String {
    let read = resp.pages.iter().filter(|p| p.error.is_none()).count();
    let mut s = format!(
        "Searched \"{}\": {} results, {} pages read, {}.{}s\n",
        resp.query,
        resp.results.len(),
        read,
        resp.took_ms / 1000,
        resp.took_ms % 1000 / 100
    );
    for r in &resp.results {
        let page = resp.pages.iter().enumerate().find(|(_, p)| p.url == r.url);
        s.push('\n');
        let _ = writeln!(s, "## {}. {}", r.rank, r.title.trim());
        let _ = writeln!(s, "{}", r.url);
        s.push_str(&domain(&r.url));
        if let Some(p) = r.published.as_deref().filter(|p| !p.trim().is_empty()) {
            let _ = write!(s, " · {}", p.trim());
        }
        if r.providers.len() > 1 {
            let _ = write!(s, " · {}", r.providers.join(", "));
        }
        if page.is_some_and(|(_, p)| p.tier == "render") {
            s.push_str(" · rendered");
        }
        s.push('\n');
        if !r.snippet.trim().is_empty() {
            let _ = writeln!(s, "{}", r.snippet.trim());
        }
        let Some((i, page)) = page else {
            continue;
        };
        if let Some(e) = &page.error {
            let _ = writeln!(s, "(could not read: {e})");
        } else if i < keep {
            for ex in &page.excerpts {
                let _ = writeln!(s, "> {}", ex.trim().replace('\n', "\n> "));
            }
        } else if !page.excerpts.is_empty() {
            let _ = writeln!(s, "[{} excerpts omitted]", page.excerpts.len());
        }
    }
    if resp.results.is_empty() && resp.features.is_empty() {
        s.push_str("\nNo results.\n");
    }
    if !resp.features.is_empty() {
        s.push_str("\nFeatures:\n");
        for kind in FeatureKind::ORDER {
            for f in resp.features.iter().filter(|f| f.kind == kind) {
                s.push_str(&cap(&feature_block(f), feature_max_chars));
                s.push('\n');
            }
        }
    }
    if !resp.failed.is_empty() {
        let lines: Vec<String> = resp
            .failed
            .iter()
            .map(|(n, e)| format!("{n} ({e})"))
            .collect();
        let _ = write!(s, "\nFailed: {}", lines.join("; "));
    }
    s.trim_end().to_owned()
}

/// The `read_url` text: `# title`, the final URL, a blank line, the content,
/// and a truncation marker when the page was cut.
#[must_use]
pub fn fetch_text(r: &FetchResponse) -> String {
    let mut s = String::new();
    if let Some(t) = r.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = writeln!(s, "# {}", t.trim());
    }
    let _ = writeln!(s, "{}\n", r.url);
    s.push_str(r.content.trim_end());
    if r.truncated {
        let _ = write!(s, "\n[truncated at {} chars]", r.content.chars().count());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{RankedResult, ReadPage};
    use crate::provider::FeatureItem;

    fn hit(rank: u32, title: &str, url: &str, snippet: &str) -> RankedResult {
        RankedResult {
            rank,
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            providers: vec!["a".into()],
            published: None,
        }
    }

    fn page(url: &str, excerpts: &[&str]) -> ReadPage {
        ReadPage {
            url: url.into(),
            title: None,
            excerpts: excerpts.iter().map(|e| (*e).to_owned()).collect(),
            error: None,
            took_ms: 0,
            tier: "fetch".into(),
        }
    }

    fn resp() -> SearchResponse {
        SearchResponse {
            query: "q".into(),
            took_ms: 1900,
            results: vec![RankedResult {
                rank: 1,
                title: "Rust".into(),
                url: "https://www.rust-lang.org/".into(),
                snippet: "A language".into(),
                providers: vec!["a".into(), "b".into()],
                published: Some("2026-01-02".into()),
            }],
            features: vec![
                Feature {
                    kind: FeatureKind::Related,
                    provider: "a".into(),
                    items: vec![FeatureItem {
                        title: "rust book".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Feature {
                    kind: FeatureKind::Answer,
                    provider: "a".into(),
                    text: Some("x".repeat(50)),
                    ..Default::default()
                },
            ],
            ok: vec!["a".into(), "b".into()],
            failed: vec![("c".into(), "rate limited".into())],
            pages: Vec::new(),
        }
    }

    #[test]
    fn header_sections_features_failed_in_order() {
        let s = search_text(&resp(), 20);
        assert!(
            s.starts_with(
                "Searched \"q\": 1 results, 0 pages read, 1.9s\n\n## 1. Rust\nhttps://www.rust-lang.org/\nrust-lang.org · 2026-01-02 · a, b\nA language\n\nFeatures:\n"
            ),
            "{s}"
        );
        let answer = s.find("[answer]").expect("answer");
        let related = s.find("[related]").expect("related");
        assert!(answer < related, "answer renders before related");
        assert!(s.ends_with("Failed: c (rate limited)"));
        let line = s.lines().find(|l| l.contains("[answer]")).expect("line");
        assert!(line.ends_with('…') && line.chars().count() <= 21, "{line}");
    }

    #[test]
    fn read_pages_quote_excerpts_and_failures_say_so() {
        let mut r = resp();
        r.features.clear();
        r.failed.clear();
        r.results = vec![
            hit(1, "A", "https://a.example/", "sa"),
            hit(2, "B", "https://b.example/", "sb"),
            hit(3, "C", "https://c.example/", "sc"),
        ];
        r.pages = vec![
            page("https://a.example/", &["first para", "second\nline"]),
            ReadPage {
                error: Some("timed out".into()),
                ..page("https://b.example/", &[])
            },
        ];
        let s = search_text(&r, 600);
        assert_eq!(
            s,
            "Searched \"q\": 3 results, 1 pages read, 1.9s\n\
             \n## 1. A\nhttps://a.example/\na.example\nsa\n> first para\n> second\n> line\n\
             \n## 2. B\nhttps://b.example/\nb.example\nsb\n(could not read: timed out)\n\
             \n## 3. C\nhttps://c.example/\nc.example\nsc"
        );
    }

    #[test]
    fn rendered_pages_are_marked_on_the_domain_line() {
        let mut r = resp();
        r.features.clear();
        r.failed.clear();
        r.results = vec![
            hit(1, "A", "https://example.com/a", "sa"),
            hit(2, "B", "https://b.example/", "sb"),
        ];
        r.pages = vec![
            ReadPage {
                tier: "render".into(),
                ..page("https://example.com/a", &["para"])
            },
            page("https://b.example/", &["para"]),
        ];
        let s = search_text(&r, 600);
        assert!(s.contains("\nexample.com · rendered\nsa\n"), "{s}");
        assert!(s.contains("\nb.example\nsb\n"), "{s}");
    }

    #[test]
    fn over_cap_drops_last_pages_excerpts_first() {
        let mut r = resp();
        r.features.clear();
        r.failed.clear();
        r.results = vec![
            hit(1, "A", "https://a.example/", ""),
            hit(2, "B", "https://b.example/", ""),
        ];
        let big_a = "a".repeat(40_000);
        let big_b = "b".repeat(30_000);
        r.pages = vec![
            page("https://a.example/", &[&big_a]),
            page("https://b.example/", &[&big_b, "tail"]),
        ];
        let s = search_text(&r, 600);
        assert!(s.chars().count() <= SEARCH_MAX_CHARS);
        assert!(
            s.contains(&format!("> {big_a}\n")),
            "first page keeps its excerpt"
        );
        assert!(!s.contains(&big_b), "last page loses its excerpts");
        assert!(
            s.ends_with("## 2. B\nhttps://b.example/\nb.example\n[2 excerpts omitted]"),
            "{}",
            &s[s.len() - 80..]
        );
    }

    #[test]
    fn nothing_found_says_no_results() {
        let r = SearchResponse {
            query: "q".into(),
            took_ms: 40,
            ..SearchResponse::default()
        };
        assert_eq!(
            search_text(&r, 600),
            "Searched \"q\": 0 results, 0 pages read, 0.0s\n\nNo results."
        );
    }

    #[test]
    fn fetch_text_marks_truncation() {
        let r = FetchResponse {
            url: "https://a".into(),
            title: Some("T".into()),
            content: "abc".into(),
            truncated: true,
            took_ms: 0,
        };
        assert_eq!(
            fetch_text(&r),
            "# T\nhttps://a\n\nabc\n[truncated at 3 chars]"
        );
    }
}
