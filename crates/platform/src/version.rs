//! The product version: the `theuth` release a binary was built as. Library
//! crates keep a frozen package version so that a release bump rebuilds only
//! the binaries (`scripts/bump-version.sh`); they read this instead of their
//! own `CARGO_PKG_VERSION`. Each binary calls [`set`] first thing in `main`.

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// Record the product version; the first call wins.
pub fn set(version: &'static str) {
    let _ = VERSION.set(version);
}

/// The product version, or the frozen library version when no binary set
/// one (tests, tools).
pub fn get() -> &'static str {
    VERSION.get().copied().unwrap_or(env!("CARGO_PKG_VERSION"))
}
