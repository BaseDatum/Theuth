//! Query-relevant passage selection for `search_web` (design doc 16 §4.7
//! "passage selection"): a read page's markdown is split into paragraphs,
//! ranked by BM25 against the query, and the best few are returned in
//! document order under a character budget. Local and dependency-free; this
//! beat a hosted reranker (LibreChat's Cohere/Jina step) because the pages
//! are already in memory and a page's own paragraphs are a good enough corpus.

const K1: f64 = 1.2;
const B: f64 = 0.75;
/// Adjacent blocks are merged until a passage reaches this many chars.
const MERGE_MIN: usize = 200;
/// A single passage is cut at this many chars.
const BLOCK_MAX: usize = 1200;
/// Blocks shorter than this (headings aside) are boilerplate, dropped.
const MIN_BLOCK: usize = 40;
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "are", "was", "were", "be",
    "been", "with", "by", "as", "at", "it", "its", "this", "that", "these", "those", "from",
    "what", "which", "who", "how", "why", "when", "where", "best", "top", "vs", "do", "does",
    "can", "not", "about", "into", "than", "then", "there", "their", "you", "your", "we", "our",
];

/// Paragraphs of `markdown` most relevant to `query`, in document order.
///
/// Paragraphs (blank-line separated blocks, nav/image/rule noise dropped,
/// headings folded into the paragraph they introduce, short neighbours merged
/// to about [`MERGE_MIN`] chars, each capped at [`BLOCK_MAX`] chars with a
/// trailing `…`) are scored by BM25 over the page's own paragraphs and taken
/// greedily by score while they fit `max_chars`. With no query token anywhere
/// on the page the leading paragraphs are returned instead. Non-blank input
/// always yields at least one passage; if none fits, the first is truncated
/// to `max_chars`. Design doc 16 §4.7.
#[must_use]
pub fn best_passages(markdown: &str, query: &str, max_chars: usize) -> Vec<String> {
    let paras = paragraphs(markdown);
    if paras.is_empty() {
        let raw = markdown.trim();
        return if raw.is_empty() {
            Vec::new()
        } else {
            vec![cut(raw, max_chars.min(BLOCK_MAX))]
        };
    }
    let scores = bm25(&paras, query);
    let matched = scores.iter().any(|s| *s > 0.0);
    let mut order: Vec<usize> = (0..paras.len()).collect();
    if matched {
        order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]).then(a.cmp(b)));
    }
    let mut chosen = Vec::new();
    let mut used = 0;
    for &i in &order {
        if matched && scores[i] <= 0.0 {
            break; // sorted: only unrelated paragraphs remain
        }
        let len = paras[i].chars().count();
        if used + len <= max_chars {
            chosen.push(i);
            used += len;
        }
    }
    if chosen.is_empty() {
        let first = &paras[order[0]];
        return vec![if first.chars().count() <= max_chars * 3 / 2 {
            first.clone()
        } else {
            cut(first, max_chars)
        }];
    }
    chosen.sort_unstable();
    chosen.into_iter().map(|i| paras[i].clone()).collect()
}

/// Total chars of the paragraphs [`best_passages`] would score: the page's
/// prose after nav / image / link-list noise and short blocks are dropped and
/// headings folded. 0 for a page that is only navigation, which is how the
/// scrape engine tells a client-rendered shell from a real article even when
/// the shell's link lists run to thousands of chars of markdown.
#[must_use]
pub fn prose_chars(markdown: &str) -> usize {
    paragraphs(markdown).iter().map(|p| p.chars().count()).sum()
}

/// Blank-line separated blocks, noise removed, headings folded, merged and capped.
fn paragraphs(markdown: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in markdown.lines().map(str::trim) {
        if line.is_empty() {
            if !cur.is_empty() {
                blocks.push(std::mem::take(&mut cur));
            }
        } else {
            if !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(&clean_line(line));
        }
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }

    let mut out = Vec::new();
    let mut acc = String::new();
    let mut heading: Option<String> = None;
    for block in blocks {
        if is_noise(&block) {
            continue;
        }
        if block.starts_with('#') && !block.contains('\n') {
            heading = Some(block.trim_start_matches('#').trim().to_owned());
            continue;
        }
        let body = match heading.take() {
            Some(h) => format!("{h} — {block}"),
            None if block.chars().count() < MIN_BLOCK => continue,
            None => block,
        };
        if !acc.is_empty() {
            acc.push('\n');
        }
        acc.push_str(&body);
        if acc.chars().count() >= MERGE_MIN {
            out.push(cut(&acc, BLOCK_MAX));
            acc.clear();
        }
    }
    if !acc.is_empty() {
        out.push(cut(&acc, BLOCK_MAX));
    }
    out
}

/// One line with runs of blanks collapsed and markdown link titles
/// (`(url "title")` → `(url)`) dropped: converters leave both behind and
/// neither carries meaning for the model.
fn clean_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("](") {
        out.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
        let Some(close) = rest.find(')') else { break };
        let target = &rest[..close];
        out.push_str(target.split(" \"").next().unwrap_or(target));
        rest = &rest[close..];
    }
    out.push_str(rest);
    let mut collapsed = String::with_capacity(out.len());
    let mut blank = false;
    for c in out.chars() {
        if c == ' ' || c == '\t' {
            if !blank {
                collapsed.push(' ');
            }
            blank = true;
        } else {
            collapsed.push(c);
            blank = false;
        }
    }
    collapsed
}

/// Horizontal rules, image-only blocks, link lists without a sentence, and
/// blocks that are mostly link targets (reference lists, affiliate rows).
fn is_noise(block: &str) -> bool {
    let rule = block
        .lines()
        .all(|l| l.len() >= 3 && l.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')));
    let image = block.lines().all(|l| l.starts_with("!["));
    let links = block
        .lines()
        .all(|l| l.starts_with("- [") || l.starts_with("* [") || l.starts_with('['))
        && !has_sentence_end(block);
    rule || image || links || link_density(block) > 0.5
}

/// Share of `block`'s chars inside `](…)` link targets.
fn link_density(block: &str) -> f64 {
    let mut inside = 0usize;
    let mut rest = block;
    while let Some(i) = rest.find("](") {
        rest = &rest[i + 2..];
        let Some(close) = rest.find(')') else { break };
        inside += rest[..close].chars().count();
        rest = &rest[close..];
    }
    let total = block.chars().count().max(1);
    #[allow(clippy::cast_precision_loss)]
    let density = inside as f64 / total as f64;
    density
}

/// `.`, `!` or `?` followed by whitespace or the end of the text.
fn has_sentence_end(s: &str) -> bool {
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if matches!(c, '.' | '!' | '?') && it.peek().is_none_or(|n| n.is_whitespace()) {
            return true;
        }
    }
    false
}

/// Truncate to `cap` chars at the last sentence end past the halfway mark,
/// else at the last whitespace, and append `…`. Shorter text is returned as is.
fn cut(text: &str, cap: usize) -> String {
    let Some((end, _)) = text.char_indices().nth(cap) else {
        return text.to_owned();
    };
    let head = &text[..end];
    let at = head
        .rfind(['.', '!', '?'])
        .filter(|&i| i >= end / 2)
        .map(|i| i + 1)
        .or_else(|| head.rfind(char::is_whitespace))
        .unwrap_or(end);
    let mut s = head[..at].trim_end().to_owned();
    s.push('…');
    s
}

/// Lowercased alphanumeric runs of two or more chars, stopwords removed; the
/// same tokenizer `search_web` uses to pick which hits are worth reading.
#[must_use]
pub fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(str::to_lowercase)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
}

/// BM25 of each paragraph against `query`, the paragraph set as the corpus,
/// plus 0.1 per query token when the paragraph contains the literal phrase.
fn bm25(paras: &[String], query: &str) -> Vec<f64> {
    let mut q = tokens(query);
    q.sort_unstable();
    q.dedup();
    if q.is_empty() {
        return vec![0.0; paras.len()];
    }
    let docs: Vec<Vec<String>> = paras.iter().map(|p| tokens(p)).collect();
    let n = docs.len() as f64;
    let avg = docs.iter().map(Vec::len).sum::<usize>() as f64 / n;
    let idf: Vec<f64> = q
        .iter()
        .map(|t| {
            let df = docs.iter().filter(|d| d.contains(t)).count() as f64;
            ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
        })
        .collect();
    let phrase = query.trim().to_lowercase();
    let bonus = 0.1 * q.len() as f64;
    docs.iter()
        .zip(paras)
        .map(|(d, p)| {
            let len = d.len() as f64;
            let mut s = 0.0;
            for (t, idf) in q.iter().zip(&idf) {
                let tf = d.iter().filter(|w| *w == t).count() as f64;
                if tf > 0.0 {
                    s += idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * len / avg));
                }
            }
            if !phrase.is_empty() && p.to_lowercase().contains(&phrase) {
                s += bonus;
            }
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ~240-char paragraph about `topic` with no other distinctive words.
    fn filler(topic: &str) -> String {
        format!("The {topic} report covers many things today. ")
            .repeat(6)
            .trim_end()
            .to_owned()
    }

    fn doc(paras: &[String]) -> String {
        paras.join("\n\n")
    }

    #[test]
    fn deep_match_beats_leading_noise() {
        let mut paras: Vec<String> = ["weather", "sports", "traffic", "markets", "gardening"]
            .iter()
            .map(|t| filler(t))
            .collect();
        paras.push(format!(
            "{} The borrow checker enforces lifetimes at compile time.",
            filler("compiler")
        ));
        let got = best_passages(&doc(&paras), "borrow checker", 600);
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("borrow checker"), "{got:?}");
    }

    #[test]
    fn document_order_not_score_order() {
        let a = format!(
            "{} One mention of the borrow checker here.",
            filler("intro")
        );
        let b = filler("unrelated");
        let c = "Borrow checker, borrow checker, borrow checker: the borrow checker rules \
                 every reference and the borrow checker never sleeps, so learn the borrow \
                 checker well before writing unsafe code or fighting lifetimes in structs."
            .to_owned();
        let text = doc(&[a.clone(), b, c.clone()]);
        // Budget for one: the higher-scoring later paragraph wins.
        assert_eq!(best_passages(&text, "borrow checker", 300), vec![c.clone()]);
        // Budget for both: returned in document order.
        assert_eq!(best_passages(&text, "borrow checker", 2000), vec![a, c]);
    }

    #[test]
    fn no_match_falls_back_to_leading_paragraphs() {
        let paras: Vec<String> = ["alpha", "beta", "gamma"]
            .iter()
            .map(|t| filler(t))
            .collect();
        let got = best_passages(&doc(&paras), "zebra quokka", 600);
        assert_eq!(got, vec![paras[0].clone(), paras[1].clone()]);
    }

    #[test]
    fn budget_is_respected() {
        let paras: Vec<String> = (0..20)
            .map(|i| format!("{} Borrow checker note {i}.", filler("rust")))
            .collect();
        let got = best_passages(&doc(&paras), "borrow checker", 1000);
        assert!(!got.is_empty());
        let total: usize = got.iter().map(|p| p.chars().count()).sum();
        assert!(total <= 1000, "{total}");
    }

    #[test]
    fn nav_links_and_images_are_dropped() {
        let text = format!(
            "![logo](https://example.com/logo.png)\n\n\
             - [Home](https://example.com/)\n- [Docs](https://example.com/docs)\n\
             - [Blog](https://example.com/blog)\n- [Pricing](https://example.com/pricing)\n\n\
             ---\n\n{}",
            filler("product")
        );
        let got = best_passages(&text, "product report", 2000);
        assert_eq!(got.len(), 1);
        assert!(
            !got[0].contains("[Home]") && !got[0].contains("![logo]"),
            "{got:?}"
        );
        assert!(got[0].starts_with("The product report"));
    }

    #[test]
    fn heading_folds_into_following_paragraph() {
        let text = format!("## Installation\n\n{}", filler("setup"));
        let got = best_passages(&text, "installation setup", 2000);
        assert_eq!(got.len(), 1);
        assert!(
            got[0].starts_with("Installation — The setup report"),
            "{got:?}"
        );
    }

    #[test]
    fn nothing_fits_truncates_first_at_sentence_end() {
        let text = filler("long");
        let got = best_passages(&text, "long report", 100);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("today.…"), "{got:?}");
        assert!(got[0].chars().count() <= 101);
    }

    #[test]
    fn blank_input_is_empty() {
        assert!(best_passages("", "anything", 500).is_empty());
        assert!(best_passages("  \n\n ", "anything", 500).is_empty());
    }

    #[test]
    fn prose_chars_ignores_navigation() {
        let nav = "![logo](https://example.com/logo.png)\n\n\
                   - [Home](https://example.com/)\n- [Reviews](https://example.com/reviews)\n\
                   - [Blog](https://example.com/blog)\n- [Pricing](https://example.com/pricing)\n\n\
                   ---\n\n[Sign in](https://example.com/login) · [Register](https://example.com/join)";
        assert_eq!(prose_chars(nav), 0);
        let article = doc(&[filler("battery"), filler("display")]);
        let got = prose_chars(&format!("{nav}\n\n{article}"));
        let len = article.chars().count();
        assert!(got > 0 && got.abs_diff(len) <= 2, "{got} vs {len}");
    }
}
