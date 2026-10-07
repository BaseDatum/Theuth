//! JSON data islands (design doc 122 D9): prose recovered from `__NEXT_DATA__`,
//! `application/json` scripts, `ld+json` `articleBody` and SvelteKit payloads
//! without evaluating JavaScript. Contract stub — `None` until W2 lands the walk.

/// Prose recovered from JSON data islands, as paragraphs in document order; `None` when nothing
/// qualifies. No JavaScript is evaluated.
///
/// Contract stub (122 §7.1): `None`.
#[must_use]
pub fn extract_islands(_html: &str) -> Option<String> {
    None
}
