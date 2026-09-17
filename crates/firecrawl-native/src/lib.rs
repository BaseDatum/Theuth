//! Vendored Firecrawl native HTML, crawler, PDF and document heuristics
//! (design doc 38). Copied from `apps/api/native` and stripped of the napi
//! layer; the tuned logic and its tests are otherwise intact.
#![allow(missing_docs)]
#![allow(clippy::all)]

pub use crate::crawler::*;
pub use crate::document::*;
pub use crate::engpicker::*;
pub use crate::html::*;
pub use crate::logging::*;
pub use crate::pdf::*;
pub use crate::utils::*;

mod crawler;
mod document;
mod engpicker;
mod html;
mod logging;
mod pdf;
mod utils;

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
