//! Main-content selection (design doc 122 D8): scored candidate with two
//! invariants when no `<main>`/`<article>` exists. Contract stub — returns the
//! input unchanged until W2 lands the scorer; the engine keeps its old
//! "empty main → whole page" fallback until then.

use serde::Serialize;

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

/// Run after the strip list on the already-stripped document; `original` is the untouched page
/// (for `<noscript>` recovery, which `transform_html` detaches early).
///
/// Contract stub (122 §7.1): `Selection { html: input, scoped: false, method: Body }`.
#[must_use]
pub fn select_main(html: &str, _original: &str, _opts: &SelectOptions) -> Selection {
    Selection {
        html: html.to_owned(),
        scoped: false,
        method: Method::Body,
    }
}
