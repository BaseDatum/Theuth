//! Tool text (design doc 16 §2, §4.4): ranked list, then `Features:` blocks in
//! [`FeatureKind::ORDER`] each capped at `feature_max_chars`, then `Fetched:`
//! pages, then one `Failed:` line. The same data goes out as JSON in
//! `ToolOutput::structured`; this is the model-facing half.

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

/// The `search_web` text. `fetched` pairs each top URL with its page text or
/// the failure to fetch it.
#[must_use]
pub fn search_text(
    resp: &SearchResponse,
    feature_max_chars: usize,
    fetched: &[(String, Result<FetchResponse, String>)],
) -> String {
    let mut s = String::new();
    for r in &resp.results {
        let _ = write!(s, "{}. {} — {}", r.rank, r.title.trim(), r.url);
        if r.providers.len() > 1 {
            let _ = write!(s, " [{}]", r.providers.join(", "));
        }
        s.push('\n');
        if !r.snippet.trim().is_empty() {
            let _ = writeln!(s, "   {}", r.snippet.trim());
        }
    }
    if resp.results.is_empty() && resp.features.is_empty() {
        s.push_str("No results.\n");
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
    if !fetched.is_empty() {
        s.push_str("\nFetched:\n");
        for (url, page) in fetched {
            let _ = writeln!(s, "## {url}");
            match page {
                Ok(p) => {
                    s.push_str(p.content.trim());
                    if p.truncated {
                        s.push_str(" […]");
                    }
                }
                Err(e) => {
                    let _ = write!(s, "(fetch failed: {e})");
                }
            }
            s.push('\n');
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
    use crate::aggregate::RankedResult;
    use crate::provider::FeatureItem;

    fn resp() -> SearchResponse {
        SearchResponse {
            query: "q".into(),
            took_ms: 1,
            results: vec![RankedResult {
                rank: 1,
                title: "Rust".into(),
                url: "https://rust-lang.org/".into(),
                snippet: "A language".into(),
                providers: vec!["a".into(), "b".into()],
                published: None,
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
        }
    }

    #[test]
    fn sections_in_order_and_feature_capped() {
        let s = search_text(&resp(), 20, &[]);
        let answer = s.find("[answer]").expect("answer");
        let related = s.find("[related]").expect("related");
        assert!(answer < related, "answer renders before related");
        assert!(s.starts_with("1. Rust — https://rust-lang.org/ [a, b]\n   A language"));
        assert!(s.ends_with("Failed: c (rate limited)"));
        let line = s.lines().find(|l| l.contains("[answer]")).expect("line");
        assert!(line.ends_with('…') && line.chars().count() <= 21, "{line}");
    }

    #[test]
    fn fetched_pages_and_failures_inline() {
        let page = FetchResponse {
            url: "https://a".into(),
            title: None,
            content: "body".into(),
            truncated: false,
            took_ms: 0,
        };
        let s = search_text(
            &resp(),
            600,
            &[
                ("https://a".into(), Ok(page)),
                ("https://b".into(), Err("boom".into())),
            ],
        );
        assert!(s.contains("Fetched:\n## https://a\nbody\n## https://b\n(fetch failed: boom)"));
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
