//! Shared human-search primitives (design doc 49 §4.1): the nucleo query
//! policy every owner applies, prepared immutable candidate text, snippets
//! with Unicode-scalar highlight ranges, and the bounded result page each
//! category answers with. No plugin, actor, store or source trait lives
//! here; agent, artifact and knowledge keep their own records and collapse
//! rules and only share what must behave identically.

use std::sync::Arc;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};

/// Hits per category when the caller omits `limit`.
pub const DEFAULT_LIMIT: u32 = 10;

/// Hard cap on hits per category; a caller asking for more gets this.
pub const MAX_LIMIT: u32 = 50;

/// Longest accepted query, in Unicode scalar values.
pub const MAX_QUERY_CHARS: usize = 1024;

/// Reject an over-long query. Blank queries pass: the caller returns an
/// empty ready page without touching any cache.
pub fn validate_query(query: &str) -> Result<(), String> {
    let n = query.chars().count();
    if n > MAX_QUERY_CHARS {
        return Err(format!(
            "query is {n} characters; the maximum is {MAX_QUERY_CHARS}"
        ));
    }
    Ok(())
}

/// The per-category limit for a request: [`DEFAULT_LIMIT`] when omitted,
/// clamped to `1..=MAX_LIMIT`.
pub fn limit(requested: Option<u32>) -> u32 {
    requested.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// The selected categories in canonical order, deduplicated. `None` means
/// all six; `Some(vec![])` means none.
pub fn categories(requested: Option<Vec<SearchCategory>>) -> Vec<SearchCategory> {
    match requested {
        None => SearchCategory::ALL.to_vec(),
        Some(wanted) => SearchCategory::ALL
            .into_iter()
            .filter(|c| wanted.contains(c))
            .collect(),
    }
}

/// One of the six search groups (49 §2.1; 66 §10 adds notes and meetings).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SearchCategory {
    /// Saved session titles and transcript text.
    Sessions,
    /// Saved artifact records and their content.
    Artifacts,
    /// Current indexed concepts.
    Knowledge,
    /// Recorded concept changes and their historical text.
    KnowledgeHistory,
    /// Free-standing notes: titles and derived markdown (66 §10).
    Notes,
    /// Meetings: titles and transcript segments (66 §10).
    Meetings,
}

impl SearchCategory {
    /// Every category, in display and wire order.
    pub const ALL: [SearchCategory; 6] = [
        SearchCategory::Sessions,
        SearchCategory::Artifacts,
        SearchCategory::Knowledge,
        SearchCategory::KnowledgeHistory,
        SearchCategory::Notes,
        SearchCategory::Meetings,
    ];
}

/// Whether a page's hits are usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    /// `hits` are the ranked top results.
    Ready,
    /// The owner is building its cache; ask again shortly.
    Indexing,
    /// The owner is disabled or its source is missing.
    Unavailable,
    /// The owner failed; `notices` says why.
    Error,
}

/// How much of the selected source a ready page actually searched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// Every eligible record was considered.
    Complete,
    /// Some records or content could not be searched; see `notices`.
    Partial,
}

/// A structured, bounded reason attached to a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchNotice {
    /// Stable machine code (`error`, `unavailable`, owner-specific codes).
    pub code: String,
    /// Human-readable reason.
    pub message: String,
    /// Records affected, when counted.
    pub affected: Option<u32>,
}

/// One category's answer: a bounded, server-ranked top-result view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchPage<T> {
    /// Whether `hits` are usable.
    pub state: PageState,
    /// Ranked hits; empty unless `state` is [`PageState::Ready`].
    pub hits: Vec<T>,
    /// A further distinct hit exists beyond `hits`; meaningful only when ready.
    pub has_more: bool,
    /// Whether the whole selected source was searched.
    pub coverage: Coverage,
    /// Bounded structured reasons (unavailable/error/partial coverage).
    pub notices: Vec<SearchNotice>,
}

impl<T> SearchPage<T> {
    /// A ready page from already-ranked `hits` that include the `limit + 1`
    /// lookahead: keeps `limit`, sets `has_more` from the remainder.
    pub fn ready(mut hits: Vec<T>, limit: u32) -> Self {
        let limit = limit as usize;
        let has_more = hits.len() > limit;
        hits.truncate(limit);
        SearchPage {
            state: PageState::Ready,
            hits,
            has_more,
            coverage: Coverage::Complete,
            notices: Vec::new(),
        }
    }

    /// The owner is building its cache.
    pub fn indexing() -> Self {
        Self::empty(PageState::Indexing, None)
    }

    /// The owner cannot answer at all (disabled plugin, missing source).
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::empty(
            PageState::Unavailable,
            Some(("unavailable", message.into())),
        )
    }

    /// The owner failed.
    pub fn error(message: impl Into<String>) -> Self {
        Self::empty(PageState::Error, Some(("error", message.into())))
    }

    fn empty(state: PageState, notice: Option<(&str, String)>) -> Self {
        SearchPage {
            state,
            hits: Vec::new(),
            has_more: false,
            coverage: Coverage::Partial,
            notices: notice
                .map(|(code, message)| SearchNotice {
                    code: code.to_owned(),
                    message,
                    affected: None,
                })
                .into_iter()
                .collect(),
        }
    }
}

/// `[start, end)` in Unicode scalar values of the string it accompanies —
/// what JS `Array.from(text)` indexes, not UTF-8 bytes or UTF-16 units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MatchRange {
    /// First matched scalar.
    pub start: u32,
    /// One past the last matched scalar.
    pub end: u32,
}

/// Which candidate matched, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TextMatch {
    /// The owner's name for the candidate (`title`, `body`, `source`, …).
    pub field: String,
    /// Text around the match, `…`-trimmed where cut.
    pub snippet: String,
    /// Matched spans inside `snippet`, ascending, non-overlapping.
    pub ranges: Vec<MatchRange>,
}

/// One immutable candidate text, segmented for nucleo once so every query
/// matches it without re-decoding. Cheap to clone (shared).
///
/// Non-ASCII text is segmented into its Unicode scalars, not nucleo's
/// grapheme-first-char collapse, so match indices are positions in
/// `as_str().chars()` and highlight ranges land on the right characters.
#[derive(Clone, Debug)]
pub struct SearchText(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    text: String,
    /// `None` when `text` is ASCII (its bytes are the UTF-32 view).
    chars: Option<Box<[char]>>,
}

impl SearchText {
    /// Prepare `text` for matching.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let chars = (!text.is_ascii()).then(|| text.chars().collect());
        SearchText(Arc::new(Inner { text, chars }))
    }

    /// The original text.
    pub fn as_str(&self) -> &str {
        &self.0.text
    }

    /// Length in Unicode scalars.
    pub fn len_chars(&self) -> usize {
        match &self.0.chars {
            Some(c) => c.len(),
            None => self.0.text.len(),
        }
    }

    fn utf32(&self) -> Utf32Str<'_> {
        match &self.0.chars {
            Some(c) => Utf32Str::Unicode(c),
            None => Utf32Str::Ascii(self.0.text.as_bytes()),
        }
    }
}

/// One parsed query plus nucleo's matcher scratch space. Not `Sync`; make
/// one per matching pass and reuse it across candidates.
pub struct FuzzyMatcher {
    pattern: Pattern,
    matcher: Matcher,
    indices: Vec<u32>,
}

impl FuzzyMatcher {
    /// Parse `query` with the shared policy: fzf syntax (space-separated
    /// atoms, `'exact`, `^prefix`, `suffix$`, `!not`), smart case, smart
    /// Unicode normalization.
    pub fn new(query: &str) -> Self {
        FuzzyMatcher {
            pattern: Pattern::parse(query.trim(), CaseMatching::Smart, Normalization::Smart),
            matcher: Matcher::new(Config::DEFAULT),
            indices: Vec::new(),
        }
    }

    /// No atoms survived parsing: everything would match with score 0.
    pub fn is_empty(&self) -> bool {
        self.pattern.atoms.is_empty()
    }

    /// nucleo score of `text`, or `None` when it does not match.
    pub fn score(&mut self, text: &SearchText) -> Option<u32> {
        self.pattern.score(text.utf32(), &mut self.matcher)
    }

    /// Score plus the matched scalar positions in `text`, sorted and
    /// deduplicated; valid until the next call.
    pub fn indices(&mut self, text: &SearchText) -> Option<(u32, &[u32])> {
        self.indices.clear();
        let score = self
            .pattern
            .indices(text.utf32(), &mut self.matcher, &mut self.indices)?;
        self.indices.sort_unstable();
        self.indices.dedup();
        Some((score, self.indices.as_slice()))
    }

    /// Score plus a ready-to-send [`TextMatch`] for candidate `field`, with
    /// up to `max_chars` scalars of `text` around the match.
    pub fn matched(
        &mut self,
        text: &SearchText,
        field: &str,
        max_chars: usize,
    ) -> Option<(u32, TextMatch)> {
        let (score, indices) = self.indices(text)?;
        let (snippet, ranges) = snippet(text.as_str(), indices, max_chars);
        Some((
            score,
            TextMatch {
                field: field.to_owned(),
                snippet,
                ranges,
            },
        ))
    }
}

/// Up to `max_chars` scalars of `text` centred on the span covered by
/// `indices` (scalar positions), `…` where cut on either side, and the
/// matched runs re-based to snippet offsets. Nothing is allocated for the
/// text itself beyond the returned snippet.
pub fn snippet(text: &str, indices: &[u32], max_chars: usize) -> (String, Vec<MatchRange>) {
    let n = text.chars().count();
    if n == 0 || max_chars == 0 {
        return (String::new(), Vec::new());
    }
    let mut sorted: Vec<usize> = indices
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| i < n)
        .collect();
    sorted.sort_unstable();
    sorted.dedup();

    let (start, end) = match (sorted.first(), sorted.last()) {
        (Some(&first), Some(&last)) => {
            let span = last - first + 1;
            if span >= max_chars {
                (first, (first + max_chars).min(n))
            } else {
                let pad = (max_chars - span) / 2;
                let end = (first.saturating_sub(pad) + max_chars).min(n);
                (end.saturating_sub(max_chars.min(n)), end)
            }
        }
        _ => (0, n.min(max_chars)),
    };

    let byte_at = |scalar: usize| {
        text.char_indices()
            .nth(scalar)
            .map_or(text.len(), |(b, _)| b)
    };
    let (bs, be) = (byte_at(start), byte_at(end));
    let mut out = String::with_capacity(be - bs + 6);
    let prefix = u32::from(start > 0);
    if start > 0 {
        out.push('…');
    }
    out.push_str(&text[bs..be]);
    if end < n {
        out.push('…');
    }
    let local: Vec<u32> = sorted
        .into_iter()
        .filter(|&i| i >= start && i < end)
        .map(|i| (i - start) as u32 + prefix)
        .collect();
    (out, runs(&local))
}

/// Sorted unique offsets → half-open runs of consecutive offsets.
fn runs(offsets: &[u32]) -> Vec<MatchRange> {
    let mut out = Vec::new();
    let Some(&first) = offsets.first() else {
        return out;
    };
    let (mut run_start, mut prev) = (first, first);
    for &i in &offsets[1..] {
        if i == prev + 1 {
            prev = i;
            continue;
        }
        out.push(MatchRange {
            start: run_start,
            end: prev + 1,
        });
        run_start = i;
        prev = i;
    }
    out.push(MatchRange {
        start: run_start,
        end: prev + 1,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighted(s: &str, r: MatchRange) -> String {
        s.chars()
            .skip(r.start as usize)
            .take((r.end - r.start) as usize)
            .collect()
    }

    #[test]
    fn snippet_is_windowed_around_the_match_with_ellipses() {
        let long = format!("{}needle{}", "a ".repeat(200), " b".repeat(200));
        let (s, ranges) = snippet(&long, &[400, 401, 402, 403, 404, 405], 160);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert_eq!(s.chars().count(), 162);
        assert_eq!(highlighted(&s, ranges[0]), "needle");
    }

    #[test]
    fn runs_collapse_adjacent_offsets() {
        assert_eq!(
            runs(&[0, 1, 2, 5, 6, 9]),
            vec![
                MatchRange { start: 0, end: 3 },
                MatchRange { start: 5, end: 7 },
                MatchRange { start: 9, end: 10 },
            ]
        );
    }

    /// Multi-scalar graphemes before the match must not shift the ranges:
    /// they are scalar positions in the snippet, what `Array.from` indexes.
    #[test]
    fn ranges_are_scalar_positions_past_multi_scalar_graphemes() {
        let text = SearchText::new("👨\u{200d}👩\u{200d}👧 café deploy now");
        let mut m = FuzzyMatcher::new("deploy");
        let (_, hit) = m.matched(&text, "body", 160).unwrap();
        assert_eq!(hit.ranges.len(), 1);
        assert_eq!(highlighted(&hit.snippet, hit.ranges[0]), "deploy");
        // Same text through `indices` and the free `snippet`.
        let (_, idx) = m.indices(&text).unwrap();
        let (s, ranges) = snippet(text.as_str(), idx, 2000);
        assert_eq!(s, text.as_str());
        assert_eq!(highlighted(&s, ranges[0]), "deploy");
    }

    #[test]
    fn fzf_syntax_and_smart_case_apply() {
        let mut m = FuzzyMatcher::new("dpy");
        assert!(m.score(&SearchText::new("deploy checklist")).is_some());
        assert!(m.score(&SearchText::new("Dpy")).is_some());
        let mut exact = FuzzyMatcher::new("'dpy");
        assert!(exact.score(&SearchText::new("deploy checklist")).is_none());
        assert!(exact.score(&SearchText::new("dpy")).is_some());
        let mut not = FuzzyMatcher::new("dpy !check");
        assert!(not.score(&SearchText::new("deploy checklist")).is_none());
        assert!(FuzzyMatcher::new("   ").is_empty());
    }

    #[test]
    fn page_ready_keeps_limit_and_reports_lookahead() {
        let page = SearchPage::ready((0..11).collect::<Vec<u32>>(), 10);
        assert_eq!(page.hits.len(), 10);
        assert!(page.has_more);
        let page = SearchPage::ready((0..10).collect::<Vec<u32>>(), 10);
        assert!(!page.has_more);
        assert_eq!(page.state, PageState::Ready);
    }

    #[test]
    fn request_parameters_are_canonical() {
        assert_eq!(limit(None), DEFAULT_LIMIT);
        assert_eq!(limit(Some(0)), 1);
        assert_eq!(limit(Some(1000)), MAX_LIMIT);
        assert_eq!(categories(None), SearchCategory::ALL.to_vec());
        assert!(categories(Some(vec![])).is_empty());
        assert_eq!(
            categories(Some(vec![
                SearchCategory::Knowledge,
                SearchCategory::Sessions,
                SearchCategory::Knowledge,
            ])),
            vec![SearchCategory::Sessions, SearchCategory::Knowledge]
        );
        assert!(validate_query("").is_ok());
        assert!(validate_query(&"é".repeat(MAX_QUERY_CHARS)).is_ok());
        assert!(validate_query(&"é".repeat(MAX_QUERY_CHARS + 1)).is_err());
    }
}
