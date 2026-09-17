//! Vendored Firecrawl native HTML and crawler heuristics (design doc 38).
//! Copied from `apps/api/native` and stripped of the napi layer; the tuned
//! logic and its tests are otherwise intact, with two exceptions:
//!
//! - `nodesig` was inlined into `html.rs` (the only non-subtraction edit).
//! - `document`, `pdf`, `engpicker`, `logging` and `utils` were dropped:
//!   PDFs already go through `web`'s fetcher (`tools::doc`), engine picking
//!   and logging belong to the TS host, and nothing that remains used them.
// Vendored code: upstream ships no item docs and we only subtract.
#![allow(missing_docs)]
// Vendored, upstream is edition 2021; 2024 let-chains make clippy want ~20
// nested `if let`s collapsed. Allowing beats rewriting vendored logic.
#![allow(clippy::collapsible_if)]

pub use crate::crawler::*;
pub use crate::html::*;

mod crawler;
mod html;

pub use serde::{Deserialize, Serialize};

/// Error from the native crate; carries the same `String` the napi layer used.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{reason}")]
pub struct NativeError {
    /// Message, formerly `napi::Error::reason`.
    pub reason: String,
}

impl NativeError {
    /// Wrap a message.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Result alias matching the old `napi::Result`.
pub type Result<T, E = NativeError> = std::result::Result<T, E>;

impl From<String> for NativeError {
    fn from(reason: String) -> Self {
        Self { reason }
    }
}

impl From<&str> for NativeError {
    fn from(reason: &str) -> Self {
        Self {
            reason: reason.to_owned(),
        }
    }
}

impl From<url::ParseError> for NativeError {
    fn from(e: url::ParseError) -> Self {
        Self {
            reason: e.to_string(),
        }
    }
}
