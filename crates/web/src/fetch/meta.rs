//! Page metadata (design doc 122 D5). Contract stub — `extract` returns the
//! empty struct until W1 lands the `<head>` scanner.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Metadata of a fetched page; every field `None` when the page had none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageMeta {
    /// `og:title` → `<title>`.
    pub title: Option<String>,
    /// `meta[name=description]` → `og:description`.
    pub description: Option<String>,
    /// `meta[name=author]` → `article:author` → JSON-LD `author.name`.
    pub author: Option<String>,
    /// RFC 3339 UTC, normalised by `crate::date::normalize`.
    pub published: Option<String>,
    /// `<html lang>`, trimmed and lowercased.
    pub lang: Option<String>,
    /// `link[rel=canonical]`, absolutised against `base`.
    pub canonical: Option<String>,
    /// `og:site_name`.
    pub site_name: Option<String>,
}

/// Scan the first 256 KiB of `html` for the D5 sources.
///
/// Contract stub (122 §7.1): `PageMeta::default()`.
#[must_use]
pub fn extract(_html: &str, _base: &url::Url) -> PageMeta {
    PageMeta::default()
}
