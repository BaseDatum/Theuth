//! Markdown compaction before the cap (design doc 122 D7). Contract stub —
//! identity until W4 lands the ten passes.

/// What `compact` keeps or folds.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompactOptions {
    /// `[text](url)` → `text` plus a trailing `Links:` list (used by `format: text`).
    pub fold_links: bool,
    /// Keep image lines (scrape); `false` drops them after the collapse pass.
    pub keep_images: bool,
}

/// Compacted markdown and how much went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compacted {
    /// The compacted markdown.
    pub text: String,
    /// `input.chars().count() − text.chars().count()`.
    pub removed_chars: usize,
}

/// Run the D7 passes over `markdown`, skipping fenced code verbatim.
///
/// Contract stub (122 §7.1): identity, `removed_chars: 0`.
#[must_use]
pub fn compact(markdown: &str, _opts: &CompactOptions) -> Compacted {
    Compacted {
        text: markdown.to_owned(),
        removed_chars: 0,
    }
}
